package dev.linuxlink.android.service

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
import android.provider.OpenableColumns
import android.util.Log
import dev.linuxlink.android.R
import dev.linuxlink.android.bridge.RustCore
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

        class Receive(
            override val address: String,
            override val port: Int,
            val isOffer: Boolean,
            val value: String,
            val name: String,
            val size: Long,
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
            enterForeground(current ?: progressNotification(getString(R.string.transfer_sending, 1, 1), "", 0))
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
            )
        }
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
            show(progressNotification(title, name, 0))
            val result = sendOne(job.address, job.port, uri, name) { percent ->
                show(progressNotification(title, name, percent))
            }
            sendCount++
            if (result.isSuccess) {
                sentTotal++
            } else {
                lastError = result.exceptionOrNull()?.message
                Log.w(TAG, "sending $name failed: $lastError")
            }
        }
    }

    private fun receive(job: Job.Receive) {
        val title = getString(R.string.transfer_receiving)
        show(progressNotification(title, job.name, 0))
        receiveCount++
        val result = receiveOne(job) { percent -> show(progressNotification(title, job.name, percent)) }
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
            size > 0L -> withProgress({ size }, onProgress) {
                RustCore.sendFileFd(address, port, descriptor.detachFd(), name, size)
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
            return Result.failure(IllegalStateException(getString(R.string.transfer_no_space, job.name)))
        }
        val target = DownloadTarget.create(this, job.name)
            ?: return Result.failure(IllegalStateException("cannot create ${job.name}"))
        return try {
            val saved = withProgress({ job.size }, onProgress) {
                RustCore.fetchFile(job.address, job.port, job.isOffer, job.value, target.descriptor.detachFd())
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
            .setSmallIcon(R.drawable.ic_stat_linux_link)
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
            .setSmallIcon(R.drawable.ic_stat_linux_link)
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

        /**
         * Save a file from the desktop at [address] to Downloads: the browsed
         * path [value], or the offer id [value] when [isOffer]. [size] is
         * what the desktop said it holds.
         */
        fun receive(
            context: Context,
            address: String,
            port: Int,
            isOffer: Boolean,
            value: String,
            name: String,
            size: Long,
        ) {
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_RECEIVE, true)
                .putExtra(EXTRA_IS_OFFER, isOffer)
                .putExtra(EXTRA_VALUE, value)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_SIZE, size)
            context.startForegroundService(intent)
        }
    }
}
