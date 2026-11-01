package dev.pocketdesk.android.cast

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R

/**
 * The computer asked this phone to be its camera (F6 part 2). Android lets
 * no app in the background open the camera, and the phone's owner should
 * see it happen anyway: the ask is offered in a notification, and only a
 * tap on it opens the camera ([WebcamActivity]), in [CastService], whose
 * own notification shows for as long as the camera is in use.
 */
object WebcamAsk {
    private const val TAG = "Pocketdesk"
    private const val CHANNEL = "webcam_asks"
    private const val NOTIFICATION_ID = 0x5743
    const val EXTRA_ADDRESS = "dev.pocketdesk.android.extra.WEBCAM_ADDRESS"
    const val EXTRA_PORT = "dev.pocketdesk.android.extra.WEBCAM_PORT"
    const val EXTRA_FRONT = "dev.pocketdesk.android.extra.WEBCAM_FRONT"
    const val EXTRA_HEIGHT = "dev.pocketdesk.android.extra.WEBCAM_HEIGHT"
    const val EXTRA_FPS = "dev.pocketdesk.android.extra.WEBCAM_FPS"

    /** What the computer asked for, held to what may be asked. */
    data class Ask(val front: Boolean, val height: Int, val fps: Int) {
        companion object {
            /** From the bridge's request: an unknown lens is the back one, a height or rate the nearest allowed. */
            fun of(lens: String, height: Int, fps: Int): Ask =
                Ask(
                    lens.equals("front", ignoreCase = true),
                    if (height > 900) 1080 else 720,
                    listOf(15, 24, 30).minBy { kotlin.math.abs(it - fps) },
                )
        }
    }

    /** An ask is offered only with the switch on, from the phone's own computer. */
    fun shouldOffer(enabled: Boolean, fromOwnComputer: Boolean): Boolean = enabled && fromOwnComputer

    /** Offer the camera to the computer at [address]:[port]. */
    fun offer(ctx: Context, address: String, port: Int, ask: Ask) {
        val app = ctx.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        if (!manager.areNotificationsEnabled()) {
            Log.w(TAG, "the computer asked for the camera, but this app's notifications are off")
            return
        }
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, app.getString(R.string.webcam_channel), NotificationManager.IMPORTANCE_HIGH)
                    .apply { description = app.getString(R.string.webcam_channel_description) },
            )
        }
        val open = PendingIntent.getActivity(
            app,
            0,
            Intent(app, WebcamActivity::class.java)
                .putExtra(EXTRA_ADDRESS, address)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_FRONT, ask.front)
                .putExtra(EXTRA_HEIGHT, ask.height)
                .putExtra(EXTRA_FPS, ask.fps)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val computer = HostStore.shownName(app, address)
        val lens = app.getString(if (ask.front) R.string.webcam_lens_front else R.string.webcam_lens_back)
        val notification = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(app.getString(R.string.webcam_ask_title, computer))
            .setContentText(app.getString(R.string.webcam_ask_text, lens, ask.height, ask.fps))
            .setContentIntent(open)
            .setAutoCancel(true)
            // An ask a few minutes old is not wanted any more.
            .setTimeoutAfter(2 * 60_000L)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
            .onFailure { Log.w(TAG, "could not offer the camera", it) }
    }

    /** Take the offer away (the camera is starting). */
    fun withdraw(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }
}
