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
 * The computer asked to open an app and to show this phone's screen, and
 * the screen is not shown there yet (Apps part 1). Android's consent for a
 * screen capture can only be given by the phone's owner at the phone, so
 * the ask is offered once in a notification, and only a tap on it opens
 * [ScreenForAppActivity], which asks for the consent, starts the cast and
 * opens the app. With a cast already running no consent is asked for at all:
 * one consent serves the whole connection.
 */
object ScreenForApp {
    private const val TAG = "Pocketdesk"
    private const val CHANNEL = "opens"
    private const val NOTIFICATION_ID = 0x5341
    const val EXTRA_ADDRESS = "dev.pocketdesk.android.extra.SCREEN_APP_ADDRESS"
    const val EXTRA_PORT = "dev.pocketdesk.android.extra.SCREEN_APP_PORT"
    const val EXTRA_CONTROL_PORT = "dev.pocketdesk.android.extra.SCREEN_APP_CONTROL_PORT"
    const val EXTRA_APP = "dev.pocketdesk.android.extra.SCREEN_APP_PACKAGE"

    /** How long the offer waits: an ask a few minutes old is not wanted any more. */
    private const val OFFER_MS = 2 * 60_000L

    /** Offer to show the screen of this phone to [host] and then open [pkg] (called [label]). */
    fun offer(ctx: Context, host: HostStore.Host, pkg: String, label: String) {
        val app = ctx.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        if (!manager.areNotificationsEnabled()) {
            Log.w(TAG, "the computer asked to open an app with the screen, but this app's notifications are off")
            return
        }
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, app.getString(R.string.channel_opens), NotificationManager.IMPORTANCE_HIGH),
            )
        }
        val open = PendingIntent.getActivity(
            app,
            pkg.hashCode(),
            Intent(app, ScreenForAppActivity::class.java)
                .putExtra(EXTRA_ADDRESS, host.address)
                .putExtra(EXTRA_PORT, host.port)
                .putExtra(EXTRA_CONTROL_PORT, host.controlPort)
                .putExtra(EXTRA_APP, pkg)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val computer = HostStore.shownName(app, host.address)
        val notification = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(app.getString(R.string.open_screen_title, label))
            .setContentText(app.getString(R.string.open_screen_text, computer))
            .setContentIntent(open)
            .setAutoCancel(true)
            .setTimeoutAfter(OFFER_MS)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
            .onFailure { Log.w(TAG, "could not offer to show the screen", it) }
    }

    /** Take the offer away (the screen is shown, or the ask is done). */
    fun withdraw(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }
}
