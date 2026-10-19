package dev.pocketdesk.android.files

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R

/**
 * The computer asked for a photo (F3): Android lets no background app open
 * the camera, so the phone's owner is offered it in a notification, which
 * opens [CameraActivity]; the photo taken goes to that computer.
 */
object CameraAsk {
    private const val TAG = "Pocketdesk"
    private const val CHANNEL = "camera_asks"
    private const val NOTIFICATION_ID = 0x4341
    const val EXTRA_ADDRESS = "dev.pocketdesk.android.extra.CAMERA_ADDRESS"
    const val EXTRA_PORT = "dev.pocketdesk.android.extra.CAMERA_PORT"

    /** A photo asked for is offered only with the switch on, from the phone's own computer. */
    fun shouldOffer(enabled: Boolean, fromOwnComputer: Boolean): Boolean = enabled && fromOwnComputer

    /** Offer the camera for the computer at [address]:[port]. */
    fun offer(ctx: Context, address: String, port: Int) {
        val app = ctx.applicationContext
        // Photos sent before and left in the cache go at each ask too (F3 review).
        sweep(java.io.File(app.cacheDir, "camera"), System.currentTimeMillis())
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        if (!manager.areNotificationsEnabled()) {
            Log.w(TAG, "the computer asked for a photo, but this app's notifications are off")
            return
        }
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, app.getString(R.string.camera_channel), NotificationManager.IMPORTANCE_HIGH)
                    .apply { description = app.getString(R.string.camera_channel_description) },
            )
        }
        val open = PendingIntent.getActivity(
            app,
            0,
            Intent(app, CameraActivity::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val computer = HostStore.shownName(app, address)
        val notification = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(app.getString(R.string.camera_ask_title, computer))
            .setContentText(app.getString(R.string.camera_ask_text))
            .setContentIntent(open)
            .setAutoCancel(true)
            // An ask a few minutes old is not wanted any more.
            .setTimeoutAfter(5 * 60_000L)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
            .onFailure { Log.w(TAG, "could not offer the camera", it) }
    }

    /** Take the offer away (the photo is being taken). */
    fun withdraw(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }

    /** Delete the camera folder's photos older than an hour. */
    fun sweep(dir: java.io.File, nowMs: Long) {
        val files = dir.listFiles() ?: return
        val old = stale(files.map { it.name to it.lastModified() }, nowMs)
        files.filter { it.name in old }.forEach { it.delete() }
    }

    /** Files in the camera's folder older than an hour, by name and time, to delete. */
    fun stale(files: List<Pair<String, Long>>, nowMs: Long): List<String> =
        files.filter { (_, modified) -> nowMs - modified > 60 * 60_000L }.map { it.first }

    /** Whether the switch for photos asked by the computer is on. */
    fun enabled(ctx: Context): Boolean = Prefs.cameraAsks(ctx)
}
