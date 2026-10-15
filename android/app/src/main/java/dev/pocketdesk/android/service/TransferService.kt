package dev.pocketdesk.android.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
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

        class Send(override val address: String, override val port: Int, val uris: List<Uri>) : Job

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
        ) : Job
    }

    // One transfer at a time: jobs started back to back queue here instead
    // of racing (the desktop takes two at once, and progress is per transfer).
    private val worker = Executors.newSingleThreadExecutor()

    // Start and stop are decided under this lock, so a job arriving as the
    // last one finishes is never left running out of the foreground.
    private val lock = Any()
    private var queued = 0
    private var current: Notification? = null

    // Only the worker thread touches these.
    private var sentTotal = 0
    private var sendCount = 0
    private var savedTotal = 0
    private var receiveCount = 0
    private var lastSaved: Uri? = null
    private var lastAddress = ""
    private var lastError: String? = null

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
        val job = intent?.let(::jobOf)
        synchronized(lock) {
            // A started foreground service must enter the foreground even
            // when it has nothing to do; keep what is showing if a job runs.
            enterForeground(current ?: progressNotification(getString(R.string.channel_transfers), "", 0))
            if (job == null) {
                if (queued == 0) stopSelf(startId)
                return START_NOT_STICKY
            }
            queued++
        }
        worker.execute { run(job ?: return@execute, startId) }
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
            )
        }
        androidx.core.content.IntentCompat.getParcelableExtra(intent, EXTRA_TREE, Uri::class.java)?.let { return Job.SendFolder(address, port, it) }
        val uris = intent.clipData
            ?.let { clip -> (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri } }
            .orEmpty()
        return if (uris.isEmpty()) null else Job.Send(address, port, uris)
    }

    /** Runs on [worker]; the last job queued reports everything done since. */
    private fun run(job: Job, startId: Int) {
        lastAddress = job.address
        try {
            when (job) {
                is Job.Send -> sendAll(job)
                is Job.SendFolder -> sendFolder(job)
                is Job.Receive -> receive(job)
            }
        } catch (t: Throwable) {
            lastError = t.message ?: t.toString()
            Log.e(TAG, "transfer job failed", t)
        } finally {
            finishJob(startId)
        }
    }

    private fun sendAll(job: Job.Send) {
        job.uris.forEachIndexed { index, uri ->
            val name = try {
                displayName(uri)
            } catch (_: Throwable) {
                "shared.bin"
            }
            val title = getString(R.string.transfer_sending, index + 1, job.uris.size)
            sendCounted(job.address, job.port, uri, name, null, title)
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
        val folderId = java.util.UUID.randomUUID().toString().replace("-", "")
        plan.files.forEachIndexed { index, file ->
            val uri = DocumentsContract.buildDocumentUriUsingTree(job.tree, file.id)
            val part = RustCore.FolderPart(folderId, folder, file.path)
            val title = getString(R.string.transfer_sending_folder, folder, index + 1, plan.files.size)
            sendCounted(job.address, job.port, uri, file.path.last(), part, title)
        }
        if (plan.skipped > 0 || plan.truncated) {
            lastError = getString(R.string.transfer_folder_partial, folder, plan.skipped)
            Log.w(TAG, "folder $folder: ${plan.skipped} names skipped, cut short: ${plan.truncated}")
        }
    }

    /** One file sent, retried as the settings say, and counted. */
    private fun sendCounted(
        address: String,
        port: Int,
        uri: Uri,
        name: String,
        part: RustCore.FolderPart?,
        title: String,
    ) {
        show(progressNotification(title, name, 0))
        var result = sendOne(address, port, uri, name, part) { percent ->
            show(progressNotification(title, name, percent))
        }
        // A failed file goes again a few times: the computer kept what
        // arrived, so each retry continues where the last one stopped.
        var attempt = 1
        while (result.isFailure) {
            val error = result.exceptionOrNull()?.message
            val wait = TransferRetry.delayBefore(attempt, error, Prefs.retryTransfers(this)) ?: break
            Log.i(TAG, "sending $name failed ($error); trying again in $wait s")
            show(progressNotification(getString(R.string.transfer_retrying, attempt), name, 0))
            Thread.sleep(wait * 1000)
            attempt++
            result = sendOne(address, port, uri, name, part) { percent ->
                show(progressNotification(title, name, percent))
            }
        }
        sendCount++
        if (result.isSuccess) {
            sentTotal++
        } else {
            lastError = result.exceptionOrNull()?.message
            Log.w(TAG, "sending $name failed: $lastError")
        }
    }

    /** The children of folder [id] in the picked [tree]. */
    private fun children(tree: Uri, id: String): List<FolderWalk.Entry> {
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
        }.getOrNull().orEmpty()
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
        show(progressNotification(title, job.name, 0))
        receiveCount++
        var result = receiveOne(job) { percent -> show(progressNotification(title, job.name, percent)) }
        // A broken download goes again a few times, continuing what it kept.
        var attempt = 1
        while (result.isFailure) {
            val error = result.exceptionOrNull()?.message
            val wait = TransferRetry.delayBefore(attempt, error, Prefs.retryTransfers(this)) ?: break
            Log.i(TAG, "receiving ${job.name} failed ($error); trying again in $wait s")
            show(progressNotification(getString(R.string.transfer_retrying, attempt), job.name, 0))
            Thread.sleep(wait * 1000)
            attempt++
            result = receiveOne(job) { percent -> show(progressNotification(title, job.name, percent)) }
        }
        result.onSuccess { uri ->
            savedTotal++
            lastSaved = uri
        }.onFailure {
            lastError = it.message
            Log.w(TAG, "receiving ${job.name} failed: $lastError")
        }
    }

    private fun finishJob(startId: Int) {
        synchronized(lock) {
            queued--
            if (queued > 0) return
            val done = doneNotification()
            sentTotal = 0
            sendCount = 0
            savedTotal = 0
            receiveCount = 0
            lastSaved = null
            lastError = null
            current = null
            runCatching { stopForeground(STOP_FOREGROUND_DETACH) }
            runCatching { manager().notify(NOTIFICATION_ID, done) }
            // Stops only if no job arrived after this one.
            stopSelf(startId)
        }
    }

    /** Run [transfer] while a watcher reports its percentage about twice a second. */
    private fun <T> withProgress(size: () -> Long, onProgress: (Int) -> Unit, transfer: () -> T): T {
        val watcher = Thread {
            try {
                while (true) {
                    Thread.sleep(PROGRESS_MS)
                    val (done, total) = RustCore.transferProgress()
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
        }
    }

    /** Send one file. */
    private fun sendOne(
        address: String,
        port: Int,
        uri: Uri,
        name: String,
        part: RustCore.FolderPart?,
        onProgress: (Int) -> Unit,
    ): Result<Unit> = try {
        val descriptor = contentResolver.openFileDescriptor(uri, "r")
            ?: return Result.failure(IllegalStateException("cannot open $name"))
        val size = descriptor.statSize
        when {
            size == 0L -> {
                descriptor.close()
                Result.failure(IllegalStateException("$name is empty"))
            }
            size > 0L -> {
                val source = SourceId.of(uri.toString(), size, lastModified(uri))
                withProgress({ size }, onProgress) {
                    RustCore.sendFileFd(address, port, descriptor.detachFd(), name, size, source, part)
                }
            }
            else -> {
                // A provider that cannot say how big the file is (a
                // stream): copy it so the desktop is told an exact size.
                descriptor.close()
                sendCopy(address, port, uri, name)
            }
        }
    } catch (t: Throwable) {
        Result.failure(t)
    }

    /** Download one file into Downloads/Pocketdesk; the uri to open it, if any. */
    private fun receiveOne(job: Job.Receive, onProgress: (Int) -> Unit): Result<Uri?> {
        if (job.size > MAX_RECEIVE_BYTES || !DownloadTarget.hasRoomFor(this, job.size)) {
            return refuse(job, getString(R.string.transfer_no_space, job.name))
        }
        // A part an earlier attempt kept is continued in place (T6a).
        // With retries off nothing is kept for one: no source, no record.
        val source = if (Prefs.retryTransfers(this)) job.source else ""
        val kept = source.takeIf { it.isNotEmpty() }
            ?.let { RustCore.partialLocation(job.address, it) }
            ?.let { DownloadTarget.reopen(this, it) }
        val target = kept ?: DownloadTarget.create(this, job.name)
            ?: return refuse(job, "cannot create ${job.name}")
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
                Result.success(target.viewUri)
            } else {
                // Kept for a retry only when the bridge recorded this place.
                val resumable = source.isNotEmpty() &&
                    RustCore.partialLocation(job.address, source) == target.location
                if (resumable) target.keepForRetry() else target.discard()
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
        return Result.failure(IllegalStateException(reason))
    }

    private fun sendCopy(address: String, port: Int, uri: Uri, name: String): Result<Unit> {
        val dir = File(cacheDir, "share").apply { mkdirs() }
        // The bridge names the file after its path: copy straight to a name
        // that is a plain file name here (the desktop makes it safe there).
        val safe = name.replace('/', '_').replace('\\', '_').take(120)
            .takeUnless { it.isBlank() || it == "." || it == ".." } ?: "shared.bin"
        val copy = File(dir, safe)
        return try {
            contentResolver.openInputStream(uri).use { input ->
                requireNotNull(input) { "content stream unavailable" }
                copy.outputStream().use { output -> input.copyTo(output) }
            }
            if (copy.length() == 0L) {
                Result.failure(IllegalStateException("$name is empty"))
            } else {
                RustCore.sendFile(address, port, copy.absolutePath)
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

    private fun progressNotification(title: String, name: String, percent: Int): Notification =
        Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .setContentText(name)
            .setProgress(100, percent, percent == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()

    private fun doneNotification(): Notification {
        val outcome = TransferSummary.of(sentTotal, sendCount, savedTotal, receiveCount)
        val title = when (outcome) {
            is TransferSummary.Outcome.Sent ->
                resources.getQuantityString(R.plurals.transfer_sent, outcome.count, outcome.count, lastAddress)
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
        val builder = Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .setContentText(if (failed) lastError else null)
            .setAutoCancel(true)
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
        private const val MAX_RECEIVE_BYTES = 8L * 1024 * 1024 * 1024
        private const val EXTRA_ADDRESS = "address"
        private const val EXTRA_PORT = "port"
        private const val EXTRA_RECEIVE = "receive"
        private const val EXTRA_IS_OFFER = "is_offer"
        private const val EXTRA_VALUE = "value"
        private const val EXTRA_NAME = "name"
        private const val EXTRA_SIZE = "size"
        private const val EXTRA_SOURCE = "source"
        private const val EXTRA_TREE = "tree"
        private const val KEEP_PARTIAL_HOURS = 24

        /** Send [uris] to the desktop at [address]; the caller's read grant passes on. */
        fun start(context: Context, address: String, port: Int, uris: List<Uri>) {
            val clip = android.content.ClipData.newRawUri("", uris.first())
            uris.drop(1).forEach { clip.addItem(android.content.ClipData.Item(it)) }
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.clipData = clip
            context.startForegroundService(intent)
        }

        /** Send every file under the folder [tree] the person picked (T6b). */
        fun sendFolder(context: Context, address: String, port: Int, tree: Uri) {
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_TREE, tree)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            context.startForegroundService(intent)
        }

        /**
         * Save a file from the desktop at [address] to Downloads: the browsed
         * path [value], or the offer id [value] when [isOffer]. [size] is
         * what the desktop said it holds, and [source] the file's id, so a
         * retry continues a broken download ("" for none).
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
        ) {
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_RECEIVE, true)
                .putExtra(EXTRA_IS_OFFER, isOffer)
                .putExtra(EXTRA_VALUE, value)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_SIZE, size)
                .putExtra(EXTRA_SOURCE, source)
            context.startForegroundService(intent)
        }
    }
}
