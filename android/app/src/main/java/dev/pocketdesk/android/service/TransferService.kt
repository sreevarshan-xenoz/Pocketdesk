package dev.pocketdesk.android.service

import dev.pocketdesk.android.ui.plainErrorText

import dev.pocketdesk.android.automation.PhoneEventBroadcast
import dev.pocketdesk.android.automation.PhoneEvents
import dev.pocketdesk.android.files.PhoneFilesRule
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import androidx.core.net.toUri
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import java.io.File
import java.util.concurrent.Executors

/**
 * Moves files between the phone and the computer in a foreground service,
 * so a transfer outlives the screen that started it: shared files go to the
 * computer (F7), and files from the computer are saved to Downloads (F7b).
 * One notification shows the running transfer with a progress bar, and
 * then what the batch did.
 *
 * Each file sent is streamed from the content provider's file descriptor;
 * only a provider that does not know the size is copied to the cache first.
 * Each file received is written straight into its download entry.
 */
class TransferService : Service() {
    /** One unit of work on the queue. */
    private sealed interface Job {
        val address: String
        val port: Int

        class Send(
            override val address: String,
            override val port: Int,
            val uris: List<Uri>,
            /** The computer's drive asked for it (D8): its share says so. */
            val drive: String? = null,
        ) : Job

        /** Every file under a folder the person picked (T6b). */
        class SendFolder(override val address: String, override val port: Int, val tree: Uri) : Job

        class Receive(
            override val address: String,
            override val port: Int,
            val isOffer: Boolean,
            val value: String,
            val name: String,
            val size: Long,
            /** The file's id at its source, so a retry continues it (T6a); "" for none. */
            val source: String,
            /** Where it sits in a folder the computer sends (T6b), if it does. */
            val folder: RustCore.FolderPart?,
            /** Its date where it came from, ms since 1970 (K1); 0 for none. */
            val lastModified: Long = 0L,
            /** The shared folder it goes into (N6), and the folders below it; null for Downloads. */
            val into: Pair<Int, List<String>>? = null,
        ) : Job
    }

    // One transfer at a time: jobs started back to back queue here instead
    // of racing (the desktop takes two at once, and progress is per transfer).
    private val worker = Executors.newSingleThreadExecutor()

    // Start and stop are decided under this lock, so a job arriving as the
    // last one finishes is never left running out of the foreground.
    private val lock = Any()
    private var queued = 0
    /** The newest start, a Cancel's included: Android stops the service only for it (C1 review). */
    private var lastStartId = 0
    private var current: Notification? = null

    // Only the worker thread touches these.
    private var sentTotal = 0
    private var sendCount = 0
    /** The files the current send means to send, for FILES_SENT (ecosystem 1f). */
    private var plannedSends = 0
    private var savedTotal = 0
    private var receiveCount = 0
    private var lastSaved: Uri? = null
    private var lastAddress = ""
    private var lastError: String? = null
    private var notice: String? = null

    /** The speed the progress notification shows (C1); 0 for none yet. */
    @Volatile private var speed = 0L

    // Cancel (C1): every job queued up to the press stops. Jobs are
    // numbered as they are queued, under [lock].
    private var enqueued = 0L
    @Volatile private var cancelThrough = 0L
    /** The job the worker runs now, by its number. */
    @Volatile private var runningJob = 0L
    /** Whether a cancel stopped anything since the last summary. */
    private var cancelledAny = false

    private fun stopped() = runningJob <= cancelThrough

    /** Woken by a Cancel, so a wait between attempts ends at once. */
    private val pause = Object()

    /** Wait [ms] between attempts, or until a Cancel (C1). */
    private fun pauseFor(ms: Long) {
        val until = SystemClock.elapsedRealtime() + ms
        synchronized(pause) {
            while (!stopped()) {
                val left = until - SystemClock.elapsedRealtime()
                if (left <= 0) return
                pause.wait(left)
            }
        }
    }

    private fun cancelledHere(): Result<Nothing> =
        Result.failure(java.io.IOException(TransferRetry.CANCELLED_ON_PHONE))

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // Unfinished downloads are kept a day for a retry, or not at all
        // when retries are off; what is dropped is deleted here.
        worker.execute {
            val hours = if (Prefs.retryTransfers(this)) KEEP_PARTIAL_HOURS else 0
            RustCore.sweepPartials(hours).forEach { DownloadTarget.delete(this, it) }
        }
    }

    override fun onDestroy() {
        worker.shutdownNow()
        super.onDestroy()
    }

    // Android 15 caps dataSync services; when the cap is hit, stop.
    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w(TAG, "foreground time limit reached; stopping")
        worker.shutdownNow()
        stopSelf()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        if (intent?.action == ACTION_CANCEL) {
            synchronized(lock) {
                lastStartId = startId
                cancelThrough = enqueued
                // Under the lock: a job queued after the press cannot have
                // started its call yet, so only what was asked stops.
                RustCore.cancelTransfer()
                if (queued == 0) stopSelf(startId)
            }
            synchronized(pause) { pause.notifyAll() }
            return START_NOT_STICKY
        }
        val job = intent?.let(::jobOf)
        val number: Long
        synchronized(lock) {
            // A started foreground service must enter the foreground even
            // when it has nothing to do; keep what is showing if a job runs.
            enterForeground(current ?: progressNotification(getString(R.string.channel_transfers), "", 0))
            lastStartId = startId
            if (job == null) {
                if (queued == 0) stopSelf(startId)
                return START_NOT_STICKY
            }
            queued++
            number = ++enqueued
        }
        worker.execute { run(job ?: return@execute, startId, number) }
        return START_NOT_STICKY
    }

    private fun jobOf(intent: Intent): Job? {
        val address = intent.getStringExtra(EXTRA_ADDRESS) ?: return null
        val port = intent.getIntExtra(EXTRA_PORT, 0).takeIf { it > 0 } ?: return null
        if (intent.getBooleanExtra(EXTRA_RECEIVE, false)) {
            val value = intent.getStringExtra(EXTRA_VALUE) ?: return null
            return Job.Receive(
                address,
                port,
                intent.getBooleanExtra(EXTRA_IS_OFFER, false),
                value,
                intent.getStringExtra(EXTRA_NAME) ?: "download",
                intent.getLongExtra(EXTRA_SIZE, 0L),
                intent.getStringExtra(EXTRA_SOURCE) ?: "",
                folderOf(intent),
                intent.getLongExtra(EXTRA_LAST_MODIFIED, 0L),
                intent.getIntExtra(EXTRA_INTO_ROOT, -1).takeIf { it >= 0 }?.let { root ->
                    root to intent.getStringArrayListExtra(EXTRA_INTO_PATH).orEmpty()
                },
            )
        }
        androidx.core.content.IntentCompat.getParcelableExtra(intent, EXTRA_TREE, Uri::class.java)?.let { return Job.SendFolder(address, port, it) }
        val uris = intent.clipData
            ?.let { clip -> (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri } }
            .orEmpty()
        val drive = intent.getStringExtra(EXTRA_DRIVE)?.takeIf { PhoneFilesRule.validDrive(it) }
        return if (uris.isEmpty()) null else Job.Send(address, port, uris, drive)
    }

    /**
     * The wait for a network during an outage (R6i), shared by every job,
     * since a folder from the computer is one job per file. Only [worker]
     * touches it.
     */
    private val offline = TransferRetry.OfflineBudget()

    /** Runs on [worker]; the last job queued reports everything done since. */
    private fun run(job: Job, startId: Int, number: Long) {
        runningJob = number
        lastAddress = job.address
        // A job that starts with a network ends any outage.
        if (NetworkWait.usable(this, job.address)) offline.reset()
        val (sentBefore, countBefore) = sentTotal to sendCount
        plannedSends = 0
        var threw = false
        try {
            when (job) {
                is Job.Send -> sendAll(job)
                is Job.SendFolder -> sendFolder(job)
                is Job.Receive -> receive(job)
            }
        } catch (t: Throwable) {
            threw = true
            lastError = t.message ?: t.toString()
            Log.e(TAG, "transfer job failed", t)
        } finally {
            if (job !is Job.Receive) {
                // Ecosystem 1f: how this send went, without the names. Every
                // file it meant to send and did not is failed, whatever
                // stopped it (a throw, the service's time running out).
                val sent = sentTotal - sentBefore
                val failed = TransferSummary.failedOf(plannedSends, sendCount - countBefore, sent, threw)
                PhoneEventBroadcast.send(
                    this,
                    PhoneEvents.Kind.FILES_SENT,
                    job.address,
                    mapOf("count" to sent, "failed" to failed),
                )
            }
            finishJob(startId)
        }
    }

    private fun sendAll(job: Job.Send) {
        plannedSends = job.uris.size
        job.uris.forEachIndexed { index, uri ->
            val name = try {
                displayName(uri)
            } catch (_: Throwable) {
                "shared.bin"
            }
            val title = getString(R.string.transfer_sending, index + 1, job.uris.size)
            if (stopped()) {
                cancelledAny = true
                sendCount++
                return@forEachIndexed
            }
            sendCounted(job.address, job.port, uri, name, null, title, job.drive)
        }
    }

    /**
     * Send every file under the folder the person picked (T6b), each with
     * its path under it, as one send the computer puts in a new folder.
     */
    private fun sendFolder(job: Job.SendFolder) {
        val rootId = DocumentsContract.getTreeDocumentId(job.tree)
        val folder = documentName(job.tree, rootId) ?: "Folder"
        val plan = FolderWalk.plan(rootId) { children(job.tree, it) }
        plannedSends = plan.files.size
        if (plan.files.isEmpty()) {
            // Nothing could be read or sent: a failure, not "0 sent".
            sendCount++
            lastError = getString(R.string.transfer_folder_empty, folder)
            Log.w(TAG, "folder $folder: nothing to send (${plan.skipped} left out)")
            return
        }
        val folderId = java.util.UUID.randomUUID().toString().replace("-", "")
        val streak = TransferRetry.FolderStreak()
        for ((index, file) in plan.files.withIndex()) {
            val uri = DocumentsContract.buildDocumentUriUsingTree(job.tree, file.id)
            val part = RustCore.FolderPart(folderId, folder, file.path)
            val title = getString(R.string.transfer_sending_folder, folder, index + 1, plan.files.size)
            if (stopped()) {
                // Cancelled (C1): the rest of the folder is not sent.
                cancelledAny = true
                sendCount += plan.files.size - index
                return
            }
            val result = sendCounted(job.address, job.port, uri, file.path.last(), part, title)
            if (TransferRetry.isCancel(result.exceptionOrNull()?.message)) {
                // Cancelled on either side (C1 review): the whole folder stops.
                cancelledAny = true
                sendCount += plan.files.size - index - 1
                return
            }
            if (!streak.record(result.isSuccess, result.exceptionOrNull()?.message)) {
                // The computer stopped answering: the rest are not sent.
                val left = plan.files.size - index - 1
                sendCount += left
                lastError = getString(R.string.transfer_folder_stopped, folder, left)
                Log.w(TAG, "folder $folder: stopped with $left files left (${result.exceptionOrNull()?.message})")
                return
            }
        }
        if (plan.skipped > 0 || plan.truncated) {
            notice = getString(R.string.transfer_folder_partial, folder, plan.skipped)
            Log.w(TAG, "folder $folder: ${plan.skipped} names skipped, cut short: ${plan.truncated}")
        }
    }

    /** One file sent, retried as the settings say, and counted; how it went. */
    private fun sendCounted(
        address: String,
        port: Int,
        uri: Uri,
        name: String,
        part: RustCore.FolderPart?,
        title: String,
        drive: String? = null,
    ): Result<Unit> {
        show(progressNotification(title, name, 0))
        var result = if (stopped()) {
            cancelledHere()
        } else {
            sendOne(address, port, uri, name, part, drive) { percent ->
                show(progressNotification(title, name, percent))
            }
        }
        // A failed file goes again a few times: the computer kept what
        // arrived, so each retry continues where the last one stopped. With
        // no network it first waits for one (R6i).
        var attempt = 1
        retry@ while (result.isFailure) {
            val error = result.exceptionOrNull()
            when (val next = TransferRetry.next(attempt, error, Prefs.retryTransfers(this), NetworkWait.usable(this, address), offline.waited(SystemClock.elapsedRealtime()))) {
                is TransferRetry.Next.Stop -> {
                    if (next.noNetwork) result = Result.failure(java.io.IOException(getString(R.string.transfer_no_network)))
                    break@retry
                }
                is TransferRetry.Next.WhenOnline -> {
                    Log.i(TAG, "sending $name failed (${error?.message}) with no network; waiting up to ${next.seconds} s")
                    show(progressNotification(getString(R.string.transfer_waiting_network), name, 0))
                    val started = SystemClock.elapsedRealtime()
                    val back = NetworkWait.await(this, address, next.seconds, ::stopped)
                    val ended = SystemClock.elapsedRealtime()
                    offline.add((ended - started + 999) / 1000, ended)
                    if (!back) continue@retry
                }
                is TransferRetry.Next.After -> {
                    Log.i(TAG, "sending $name failed (${error?.message}); trying again in ${next.seconds} s")
                    show(progressNotification(getString(R.string.transfer_retrying, attempt), name, 0))
                    pauseFor(next.seconds * 1000)
                    attempt++
                }
            }
            result = if (stopped()) {
                cancelledHere()
            } else {
                sendOne(address, port, uri, name, part, drive) { percent ->
                    show(progressNotification(title, name, percent))
                }
            }
        }
        sendCount++
        // A file that went before the press is sent, not cancelled.
        if (result.isFailure && stopped()) cancelledAny = true
        if (result.isSuccess) {
            sentTotal++
            offline.reset()
        } else {
            lastError = result.exceptionOrNull()?.message
            Log.w(TAG, "sending $name failed: $lastError")
        }
        return result
    }

    /** The children of folder [id] in the picked [tree]; null when the provider could not list it. */
    private fun children(tree: Uri, id: String): List<FolderWalk.Entry>? {
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, id)
        return runCatching {
            contentResolver.query(uri, projection, null, null, null)?.use { cursor ->
                buildList {
                    while (cursor.moveToNext()) {
                        add(
                            FolderWalk.Entry(
                                id = cursor.getString(0) ?: continue,
                                name = cursor.getString(1) ?: continue,
                                isDir = cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR,
                                size = if (cursor.isNull(3)) 0 else cursor.getLong(3),
                            ),
                        )
                    }
                }
            }
        }.onFailure { Log.w(TAG, "could not list a folder: ${it.message}") }.getOrNull()
    }

    /** The display name of document [id] in [tree]. */
    private fun documentName(tree: Uri, id: String): String? =
        runCatching {
            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)
            contentResolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)
                ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
        }.getOrNull()?.takeIf { FolderWalk.sendable(it) }

    private fun receive(job: Job.Receive) {
        val title = getString(R.string.transfer_receiving)
        receiveCount++
        if (stopped()) {
            cancelledAny = true
            if (job.isOffer) declineCancelled(job)
            return
        }
        show(progressNotification(title, job.name, 0))
        var result: Result<Uri?> = receiveOne(job) { percent -> show(progressNotification(title, job.name, percent)) }
        // A broken download goes again a few times, continuing what it kept;
        // with no network it first waits for one (R6i).
        var attempt = 1
        retry@ while (result.isFailure) {
            val error = result.exceptionOrNull()
            when (val next = TransferRetry.next(attempt, error, Prefs.retryTransfers(this), NetworkWait.usable(this, job.address), offline.waited(SystemClock.elapsedRealtime()))) {
                is TransferRetry.Next.Stop -> {
                    if (next.noNetwork) result = Result.failure(java.io.IOException(getString(R.string.transfer_no_network)))
                    break@retry
                }
                is TransferRetry.Next.WhenOnline -> {
                    Log.i(TAG, "receiving ${job.name} failed (${error?.message}) with no network; waiting up to ${next.seconds} s")
                    show(progressNotification(getString(R.string.transfer_waiting_network), job.name, 0))
                    val started = SystemClock.elapsedRealtime()
                    val back = NetworkWait.await(this, job.address, next.seconds, ::stopped)
                    val ended = SystemClock.elapsedRealtime()
                    offline.add((ended - started + 999) / 1000, ended)
                    if (!back) continue@retry
                }
                is TransferRetry.Next.After -> {
                    Log.i(TAG, "receiving ${job.name} failed (${error?.message}); trying again in ${next.seconds} s")
                    show(progressNotification(getString(R.string.transfer_retrying, attempt), job.name, 0))
                    pauseFor(next.seconds * 1000)
                    attempt++
                }
            }
            result = if (stopped()) cancelledHere() else receiveOne(job) { percent -> show(progressNotification(title, job.name, percent)) }
        }
        result.onSuccess { uri ->
            savedTotal++
            lastSaved = uri
            offline.reset()
            // Ecosystem 1f: its type and size, never its name.
            val mime = uri?.let { runCatching { contentResolver.getType(it) }.getOrNull() } ?: "application/octet-stream"
            PhoneEventBroadcast.send(
                this,
                PhoneEvents.Kind.FILE_RECEIVED,
                job.address,
                mapOf("mime" to mime, "size" to job.size),
            )
        }.onFailure {
            lastError = it.message
            Log.w(TAG, "receiving ${job.name} failed: $lastError")
            if (stopped()) {
                cancelledAny = true
                if (job.isOffer) declineCancelled(job)
            }
        }
    }

    /** Tell the computer an offer was cancelled here, so its Sent list says so (C1). */
    private fun declineCancelled(job: Job.Receive) {
        RustCore.declineOffer(job.address, job.port, job.value, CANCELLED_HERE)
            .onFailure { Log.w(TAG, "could not decline ${job.value}: ${it.message}") }
    }

    private fun finishJob(startId: Int) {
        synchronized(lock) {
            queued--
            if (queued > 0) return
            if (cancelledAny) notice = getString(R.string.transfer_cancelled)
            val done = doneNotification()
            cancelledAny = false
            sentTotal = 0
            sendCount = 0
            savedTotal = 0
            receiveCount = 0
            lastSaved = null
            lastError = null
            notice = null
            current = null
            runCatching { stopForeground(STOP_FOREGROUND_DETACH) }
            runCatching { manager().notify(NOTIFICATION_ID, done) }
            // The newest start (a Cancel's too): Android ignores an older
            // one, and a job that arrived since keeps it running anyway.
            stopSelf(lastStartId)
        }
    }

    /** Run [transfer] while a watcher reports its percentage about twice a second. */
    private fun <T> withProgress(size: () -> Long, onProgress: (Int) -> Unit, transfer: () -> T): T {
        val meter = TransferProgress.Speed()
        speed = 0L
        val watcher = Thread {
            try {
                while (true) {
                    Thread.sleep(PROGRESS_MS)
                    val (done, total) = RustCore.transferProgress()
                    // The bridge's counters are this file's only once they say so.
                    speed = if (size() > 0 && total == size()) meter.add(SystemClock.elapsedRealtime(), done) else 0L
                    onProgress(TransferProgress.forFile(done, total, size()))
                }
            } catch (_: InterruptedException) {
                // The file is done.
            }
        }
        watcher.start()
        try {
            return transfer()
        } finally {
            // Joined, so no late progress update can land on top of the
            // result notification.
            watcher.interrupt()
            if (watcher.isAlive) watcher.join()
            speed = 0L
        }
    }

    /** Send one file. */
    private fun sendOne(
        address: String,
        port: Int,
        uri: Uri,
        name: String,
        part: RustCore.FolderPart?,
        drive: String?,
        onProgress: (Int) -> Unit,
    ): Result<Unit> = try {
        ensureLink(address, port)
        // M5: a photo's place kept, when the person allowed it.
        val opened = dev.pocketdesk.android.files.PhotoPlacePrefs.open(this, uri) {
            contentResolver.openFileDescriptor(it, "r")
        }
        val descriptor = opened.value
            ?: return Result.failure(IllegalStateException("cannot open $name"))
        val size = descriptor.statSize
        when {
            size == 0L -> {
                descriptor.close()
                Result.failure(IllegalStateException("$name is empty"))
            }
            size > 0L -> {
                val modified = lastModified(uri)
                // The original and the place-less copy differ in their bytes:
                // never resumed one from the other (M5 review).
                val source = SourceId.of(sourceKey(uri, opened.original), size, modified)
                withProgress({ size }, onProgress) {
                    RustCore.sendFileFd(
                        address, port, descriptor.detachFd(), name, size, source, part, drive,
                        FileDates.toSend(Prefs.keepFileDates(this), modified),
                    )
                }
            }
            else -> {
                // A provider that cannot say how big the file is (a
                // stream): copy it so the desktop is told an exact size.
                descriptor.close()
                sendCopy(address, port, uri, name, drive)
            }
        }
    } catch (t: Throwable) {
        Result.failure(t)
    }

    /** Download one file into Downloads/Pocketdesk; the uri to open it, if any. */
    /**
     * A control link to the computer at [address] before an attempt (2d):
     * away from home a transfer rides it, and a share from another app, or a
     * retry after the link went, may find none up. At home it is a TLS link,
     * away the internet road's; the same call the background features make.
     */
    private fun ensureLink(address: String, port: Int) {
        if (RustCore.controlPeer() == address to port) return
        val wan = HostStore.wanIdentity(this, address) ?: return
        RustCore.connectToPeer(address, port, wan)
    }

    private fun receiveOne(job: Job.Receive, onProgress: (Int) -> Unit): Result<Uri?> {
        ensureLink(job.address, job.port)
        if (job.size > MAX_RECEIVE_BYTES || !DownloadTarget.hasRoomFor(this, job.size)) {
            return refuse(job, getString(R.string.transfer_no_space, job.name))
        }
        // N6: into a shared folder its owner marked, never anywhere else.
        job.into?.let { (root, path) ->
            val folder = dev.pocketdesk.android.files.PhoneFolders.all(this).firstOrNull { it.id == root && it.add }
                ?: return refuse(job, dev.pocketdesk.android.files.AddRule.NOT_MARKED)
            if (dev.pocketdesk.android.files.AddRule.decide(true, true, path, job.name) != dev.pocketdesk.android.files.AddRule.Decision.ADD) {
                return refuse(job, dev.pocketdesk.android.files.AddRule.BAD_NAME)
            }
            val target = DownloadTarget.createIn(this, folder.uri.toUri(), job.name, path)
                ?: return refuse(job, "cannot create ${job.name}")
            return try {
                val saved = withProgress({ job.size }, onProgress) {
                    RustCore.fetchFile(job.address, job.port, true, job.value, target.descriptor.detachFd(), "", job.name, target.location)
                }
                if (saved.isSuccess) {
                    target.publish()
                    Result.success(target.viewUri)
                } else {
                    target.discard()
                    Result.failure(saved.exceptionOrNull() ?: IllegalStateException("not saved"))
                }
            } catch (t: Throwable) {
                target.discard()
                Result.failure(t)
            }
        }
        // A file of a folder from the computer (T6b) goes under that send's
        // own new folder, at its place, every name checked again here.
        val folders = job.folder?.let { part ->
            if (!FolderWalk.receivable(part.folder, part.path)) {
                return refuse(job, "unsafe place in a folder: ${part.path.joinToString("/")}")
            }
            val top = synchronized(folderTops) {
                folderTops.getOrPut(part.folderId) { DownloadTarget.freeFolder(this, part.folder) }
            }
            listOf(top) + part.path.dropLast(1)
        }.orEmpty()
        // A part an earlier attempt kept is continued in place (T6a).
        // With retries off nothing is kept for one: no source, no record.
        val source = if (Prefs.retryTransfers(this)) job.source else ""
        val kept = source.takeIf { it.isNotEmpty() }
            ?.let { RustCore.partialLocation(job.address, it) }
            ?.let { DownloadTarget.reopen(this, it) }
        val target = kept ?: DownloadTarget.create(this, job.name, folders)
            ?: return refuse(job, "cannot create ${job.name}")
        // The picked folder can no longer be written (C2): the owner hears
        // where the file went instead.
        if (kept == null && (DownloadTarget.treeLost(this) || DownloadTarget.fellBack)) {
            notice = getString(R.string.transfer_folder_gone)
        }
        return try {
            val saved = withProgress({ job.size }, onProgress) {
                RustCore.fetchFile(
                    job.address,
                    job.port,
                    job.isOffer,
                    job.value,
                    target.descriptor.detachFd(),
                    source,
                    job.name,
                    target.location,
                )
            }
            if (saved.isSuccess) {
                target.publish()
                // Its date where it came from, where Android lets it be set (K1).
                FileDates.toKeep(Prefs.keepFileDates(this), job.lastModified, System.currentTimeMillis())
                    ?.let { if (!target.date(it)) Log.i(TAG, "${job.name} keeps its arrival date here") }
                Result.success(target.viewUri)
            } else {
                // Kept for a retry only when the bridge recorded this place.
                val resumable = source.isNotEmpty() &&
                    RustCore.partialLocation(job.address, source) == target.location
                if (TransferRetry.keepsPart(resumable, saved.exceptionOrNull()?.message)) target.keepForRetry() else target.discard()
                Result.failure(saved.exceptionOrNull() ?: IllegalStateException("not saved"))
            }
        } catch (t: Throwable) {
            target.discard()
            Result.failure(t)
        }
    }

    /**
     * Give up on [job] before fetching it. An offer is declined, so the
     * desktop's sent list says why instead of waiting on it.
     */
    private fun refuse(job: Job.Receive, reason: String): Result<Uri?> {
        if (job.isOffer) {
            RustCore.declineOffer(job.address, job.port, job.value, reason)
                .onFailure { Log.w(TAG, "could not decline ${job.value}: ${it.message}") }
        }
        return Result.failure(TransferRetry.Refusal(reason))
    }

    /** What a file's resume is keyed on: its original apart from its place-less copy (M5). */
    private fun sourceKey(uri: Uri, original: Boolean): String =
        if (original) "$uri#original" else uri.toString()

    private fun sendCopy(address: String, port: Int, uri: Uri, name: String, drive: String?): Result<Unit> {
        val dir = File(cacheDir, "share").apply { mkdirs() }
        // The bridge names the file after its path: copy straight to a name
        // that is a plain file name here (the desktop makes it safe there).
        val safe = name.replace('/', '_').replace('\\', '_').take(120)
            .takeUnless { it.isBlank() || it == "." || it == ".." } ?: "shared.bin"
        val copy = File(dir, safe)
        return try {
            val opened = dev.pocketdesk.android.files.PhotoPlacePrefs.open(this, uri) { contentResolver.openInputStream(it) }
            opened.value.use { input ->
                requireNotNull(input) { "content stream unavailable" }
                copy.outputStream().use { output -> input.copyTo(output) }
            }
            val size = copy.length()
            when {
                size == 0L -> Result.failure(IllegalStateException("$name is empty"))
                // Sent from the copy as any file is, with its date (K1
                // review: the plain send carried none) and, for one the
                // computer's drive asked for, saying so (D8 review).
                else -> {
                    val descriptor = android.os.ParcelFileDescriptor.open(copy, android.os.ParcelFileDescriptor.MODE_READ_ONLY)
                    val modified = lastModified(uri)
                    val source = SourceId.of(sourceKey(uri, opened.original), size, modified)
                    RustCore.sendFileFd(
                        address, port, descriptor.detachFd(), name, size, source, null, drive,
                        FileDates.toSend(Prefs.keepFileDates(this), modified),
                    )
                }
            }
        } finally {
            copy.delete()
        }
    }

    /** When the provider says [uri] last changed, if it says. */
    private fun lastModified(uri: Uri): Long? =
        runCatching {
            contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                if (!cursor.moveToFirst()) return@use null
                listOf(DocumentsContract.Document.COLUMN_LAST_MODIFIED, MediaStore.MediaColumns.DATE_MODIFIED)
                    .map { cursor.getColumnIndex(it) }
                    .firstOrNull { it >= 0 && !cursor.isNull(it) }
                    ?.let { column ->
                        val value = cursor.getLong(column)
                        // MediaStore counts seconds, DocumentsContract milliseconds.
                        if (cursor.getColumnName(column) == MediaStore.MediaColumns.DATE_MODIFIED) value * 1000 else value
                    }
            }
        }.getOrNull()

    private fun displayName(uri: Uri): String =
        runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
        }.getOrNull()
            ?: uri.lastPathSegment?.substringAfterLast('/')
            ?: "shared.bin"

    private fun enterForeground(notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    /** Post a progress notification, unless the batch has already finished. */
    private fun show(notification: Notification) {
        synchronized(lock) {
            if (queued == 0) return
            current = notification
            runCatching { manager().notify(NOTIFICATION_ID, notification) }
        }
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_transfers),
            NotificationManager.IMPORTANCE_LOW,
        )
        channel.setShowBadge(false)
        channel.description = getString(R.string.channel_transfers_desc)
        manager().createNotificationChannel(channel)
    }

    private fun progressNotification(title: String, name: String, percent: Int): Notification {
        val rate = speed
        val text = if (percent > 0 && rate > 0) {
            getString(R.string.transfer_name_speed, name, android.text.format.Formatter.formatShortFileSize(this, rate))
        } else {
            name
        }
        val cancel = PendingIntent.getService(
            this,
            1,
            Intent(this, TransferService::class.java).setAction(ACTION_CANCEL),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .setContentText(text)
            .setProgress(100, percent, percent == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(
                Notification.Action.Builder(
                    android.graphics.drawable.Icon.createWithResource(this, R.drawable.ic_notif_disconnect),
                    getString(R.string.cancel),
                    cancel,
                ).build(),
            )
            .build()
    }

    private fun doneNotification(): Notification {
        val outcome = TransferSummary.of(sentTotal, sendCount, savedTotal, receiveCount)
        val title = when (outcome) {
            is TransferSummary.Outcome.Sent ->
                resources.getQuantityString(
                    R.plurals.transfer_sent,
                    outcome.count,
                    outcome.count,
                    dev.pocketdesk.android.HostStore.shownName(this, lastAddress),
                )
            is TransferSummary.Outcome.Saved ->
                resources.getQuantityString(R.plurals.transfer_saved, outcome.count, outcome.count)
            is TransferSummary.Outcome.Mixed ->
                getString(R.string.transfer_done_mixed, outcome.sent, outcome.saved)
            is TransferSummary.Outcome.NotSent ->
                getString(R.string.transfer_failed, outcome.failed, outcome.count)
            is TransferSummary.Outcome.Failed ->
                getString(R.string.transfer_some_failed, outcome.failed, outcome.count)
        }
        val failed = outcome is TransferSummary.Outcome.NotSent || outcome is TransferSummary.Outcome.Failed
        val detail = TransferSummary.detail(failed, lastError?.let { plainErrorText(it) }, notice)
        val builder = Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .setContentText(detail)
            .setAutoCancel(true)
        // A detail is often longer than one line.
        detail?.let { builder.setStyle(Notification.BigTextStyle().bigText(it)) }
        // Tapping a finished download opens it.
        val saved = lastSaved
        if (!failed && saved != null) {
            val view = Intent(Intent.ACTION_VIEW)
                .setDataAndType(saved, contentResolver.getType(saved))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            builder.setContentIntent(
                PendingIntent.getActivity(this, 0, view, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
            )
        }
        return builder.build()
    }

    private fun manager(): NotificationManager = getSystemService(NotificationManager::class.java)

    companion object {
        private const val TAG = "TransferService"
        private const val CHANNEL_ID = "transfers"
        private const val NOTIFICATION_ID = 7
        private const val PROGRESS_MS = 500L
        private const val ACTION_CANCEL = "dev.pocketdesk.android.action.CANCEL_TRANSFER"
        /** What the computer's Sent list says of an offer cancelled here (C1). */
        private const val CANCELLED_HERE = TransferRetry.CANCELLED_ON_PHONE
        private const val MAX_RECEIVE_BYTES = 8L * 1024 * 1024 * 1024
        private const val EXTRA_ADDRESS = "address"
        private const val EXTRA_PORT = "port"
        private const val EXTRA_RECEIVE = "receive"
        private const val EXTRA_IS_OFFER = "is_offer"
        private const val EXTRA_VALUE = "value"
        private const val EXTRA_NAME = "name"
        private const val EXTRA_SIZE = "size"
        private const val EXTRA_SOURCE = "source"
        private const val EXTRA_LAST_MODIFIED = "last_modified"
        private const val EXTRA_TREE = "tree"
        private const val EXTRA_FOLDER_ID = "folder_id"
        private const val EXTRA_FOLDER = "folder"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_DRIVE = "drive"
        private const val EXTRA_INTO_ROOT = "into_root"
        private const val EXTRA_INTO_PATH = "into_path"

        /**
         * The top folder each folder send from the computer was saved in
         * (T6b), for the sends still arriving: the least recently used go
         * past [MAX_FOLDER_SENDS].
         */
        private val folderTops = object : LinkedHashMap<String, String>(16, 0.75f, true) {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, String>?) =
                size > MAX_FOLDER_SENDS
        }
        private const val MAX_FOLDER_SENDS = 32

        private fun folderOf(intent: Intent): RustCore.FolderPart? {
            val id = intent.getStringExtra(EXTRA_FOLDER_ID)?.takeIf { it.isNotEmpty() } ?: return null
            val folder = intent.getStringExtra(EXTRA_FOLDER) ?: return null
            val path = intent.getStringArrayListExtra(EXTRA_PATH)?.takeIf { it.isNotEmpty() } ?: return null
            return RustCore.FolderPart(id, folder, path)
        }
        private const val KEEP_PARTIAL_HOURS = 24

        /** Send [uris] to the desktop at [address]; the caller's read grant passes on. */
        fun start(context: Context, address: String, port: Int, uris: List<Uri>, drive: String? = null) {
            val clip = android.content.ClipData.newRawUri("", uris.first())
            uris.drop(1).forEach { clip.addItem(android.content.ClipData.Item(it)) }
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_DRIVE, drive)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.clipData = clip
            context.startForegroundService(intent)
        }

        /**
         * Send every file under the folder [tree] the person picked (T6b).
         * The app's own picker granted this app the tree, so the service
         * reads it with no grant passed on (one would not cover an extra).
         */
        fun sendFolder(context: Context, address: String, port: Int, tree: Uri) {
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_TREE, tree)
            context.startForegroundService(intent)
        }

        /**
         * Save a file from the desktop at [address] to Downloads: the browsed
         * path [value], or the offer id [value] when [isOffer]. [size] is
         * what the desktop said it holds, and [source] the file's id, so a
         * retry continues a broken download ("" for none). [lastModified] is
         * its date where it came from (ms, 0 for none; K1).
         */
        fun receive(
            context: Context,
            address: String,
            port: Int,
            isOffer: Boolean,
            value: String,
            name: String,
            size: Long,
            source: String,
            folder: RustCore.FolderPart? = null,
            lastModified: Long = 0L,
        ) {
            context.startForegroundService(receiveIntent(context, address, port, isOffer, value, name, size, source, folder, lastModified))
        }

        /**
         * Fetch the computer's offer [id] into the shared folder [root],
         * under [path], as [name] (N6: a file copied onto its drive).
         */
        fun receiveInto(context: Context, address: String, port: Int, id: String, name: String, size: Long, root: Int, path: List<String>) {
            val intent = receiveIntent(context, address, port, true, id, name, size, "")
                .putExtra(EXTRA_INTO_ROOT, root)
                .putStringArrayListExtra(EXTRA_INTO_PATH, ArrayList(path))
            context.startForegroundService(intent)
        }

        /** The intent that starts a download, to start now or from a notification's tap (T10d). */
        fun receiveIntent(
            context: Context,
            address: String,
            port: Int,
            isOffer: Boolean,
            value: String,
            name: String,
            size: Long,
            source: String,
            folder: RustCore.FolderPart? = null,
            lastModified: Long = 0L,
        ): Intent {
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_RECEIVE, true)
                .putExtra(EXTRA_IS_OFFER, isOffer)
                .putExtra(EXTRA_VALUE, value)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_SIZE, size)
                .putExtra(EXTRA_SOURCE, source)
                .putExtra(EXTRA_LAST_MODIFIED, lastModified)
            folder?.let {
                intent.putExtra(EXTRA_FOLDER_ID, it.folderId)
                    .putExtra(EXTRA_FOLDER, it.folder)
                    .putStringArrayListExtra(EXTRA_PATH, ArrayList(it.path))
            }
            return intent
        }
    }
}
