package dev.linuxlink.android.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
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

/**
 * Sends shared files to the desktop (F7) in a foreground service, so a
 * transfer outlives the share screen. One notification shows which file of
 * how many, with a progress bar, and then the result.
 *
 * Each file is streamed from the content provider's file descriptor; only a
 * provider that does not know the size is copied to the cache first.
 */
class TransferService : Service() {
    // One transfer at a time: shares made back to back queue here instead of
    // racing (the desktop takes two at once, and progress is per transfer).
    private val worker = java.util.concurrent.Executors.newSingleThreadExecutor()
    private val queued = java.util.concurrent.atomic.AtomicInteger(0)
    private var sentTotal = 0
    private var countTotal = 0
    private var lastError: String? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        worker.shutdownNow()
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val address = intent?.getStringExtra(EXTRA_ADDRESS)
        val port = intent?.getIntExtra(EXTRA_PORT, 0) ?: 0
        val uris = intent?.clipData?.let { clip -> (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri } }
            .orEmpty()
        if (address == null || port <= 0 || uris.isEmpty()) {
            stopSelf(startId)
            return START_NOT_STICKY
        }
        createChannel()
        val first = progressNotification(1, uris.size, "", 0)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, first, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, first)
        }
        queued.incrementAndGet()
        worker.execute { sendAll(address, port, uris, startId) }
        return START_NOT_STICKY
    }

    /** Runs on [worker]; the last job queued reports everything sent since. */
    private fun sendAll(address: String, port: Int, uris: List<Uri>, startId: Int) {
        uris.forEachIndexed { index, uri ->
            val name = displayName(uri)
            val result = sendOne(address, port, uri, name) { percent ->
                notify(progressNotification(index + 1, uris.size, name, percent))
            }
            countTotal++
            if (result.isSuccess) {
                sentTotal++
            } else {
                lastError = result.exceptionOrNull()?.message
                Log.w(TAG, "sending $name failed: $lastError")
            }
        }
        if (queued.decrementAndGet() == 0) {
            stopForeground(STOP_FOREGROUND_DETACH)
            notify(doneNotification(sentTotal, countTotal, address, lastError))
            sentTotal = 0
            countTotal = 0
            lastError = null
            // Stops only if no share arrived after this one.
            stopSelf(startId)
        }
    }

    /** Send one file, reporting its percentage about twice a second. */
    private fun sendOne(
        address: String,
        port: Int,
        uri: Uri,
        name: String,
        onProgress: (Int) -> Unit,
    ): Result<Unit> {
        var size = 0L
        val watcher = Thread {
            try {
                while (true) {
                    Thread.sleep(PROGRESS_MS)
                    val (done, total) = RustCore.transferProgress()
                    onProgress(TransferProgress.forFile(done, total, size))
                }
            } catch (_: InterruptedException) {
                // The file is done.
            }
        }
        return try {
            val descriptor = contentResolver.openFileDescriptor(uri, "r")
                ?: return Result.failure(IllegalStateException("cannot open $name"))
            size = descriptor.statSize
            watcher.start()
            if (size > 0) {
                RustCore.sendFileFd(address, port, descriptor.detachFd(), name, size)
            } else {
                // A provider that cannot say how big the file is (a stream):
                // copy it so the desktop is told an exact size.
                descriptor.close()
                sendCopy(address, port, uri, name)
            }
        } catch (e: Exception) {
            Result.failure(e)
        } finally {
            watcher.interrupt()
        }
    }

    private fun sendCopy(address: String, port: Int, uri: Uri, name: String): Result<Unit> {
        val dir = File(cacheDir, "share").apply { mkdirs() }
        val temp = File(dir, "pending.bin")
        return try {
            contentResolver.openInputStream(uri).use { input ->
                requireNotNull(input) { "content stream unavailable" }
                temp.outputStream().use { output -> input.copyTo(output) }
            }
            // The bridge names the file after the path, so give it the real name.
            val named = File(dir, name.replace('/', '_')).also { temp.renameTo(it) }
            try {
                RustCore.sendFile(address, port, named.absolutePath)
            } finally {
                named.delete()
            }
        } finally {
            temp.delete()
        }
    }

    private fun displayName(uri: Uri): String =
        runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
        }.getOrNull()
            ?: uri.lastPathSegment?.substringAfterLast('/')
            ?: "shared.bin"

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

    private fun progressNotification(index: Int, count: Int, name: String, percent: Int): Notification =
        Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_linux_link)
            .setContentTitle(getString(R.string.transfer_sending, index, count))
            .setContentText(name)
            .setProgress(100, percent, percent == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()

    private fun doneNotification(sent: Int, count: Int, address: String, error: String?): Notification {
        val title = if (sent == count) {
            resources.getQuantityString(R.plurals.transfer_sent, count, count, address)
        } else {
            getString(R.string.transfer_failed, count - sent, count)
        }
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_linux_link)
            .setContentTitle(title)
            .setContentText(if (sent == count) null else error)
            .setAutoCancel(true)
            .build()
    }

    private fun notify(notification: Notification) = manager().notify(NOTIFICATION_ID, notification)

    private fun manager(): NotificationManager = getSystemService(NotificationManager::class.java)

    companion object {
        private const val TAG = "TransferService"
        private const val CHANNEL_ID = "transfers"
        private const val NOTIFICATION_ID = 7
        private const val PROGRESS_MS = 500L
        private const val EXTRA_ADDRESS = "address"
        private const val EXTRA_PORT = "port"

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
    }
}
