package dev.pocketdesk.android.bluetooth

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.mutableStateOf
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore

/**
 * The one-time notice of the Bluetooth fallback: the first time the link
 * goes over Bluetooth, the phone says so, once and never again
 * ([Prefs.bluetoothTold]): a dialog when the app is in front, else a
 * notification, each with "Turn this off", which opens the switch in
 * Settings.
 */
object BluetoothNotice {
    private const val CHANNEL = "pocketdesk-bluetooth"
    private const val NOTIFICATION_ID = 0x42_54

    /** Set on a notification's intent: Settings opens at the switch. */
    const val EXTRA_OPEN_SETTING = "dev.pocketdesk.android.BLUETOOTH_SETTING"

    /** How the phone says it. */
    enum class Way { DIALOG, NOTIFICATION }

    /** Whether the app is on screen; [MainActivity] keeps it. */
    @Volatile
    var inFront = false

    /** The computer the dialog names while it is up, for [MainActivity] to show; null for none. */
    val pending = mutableStateOf<String?>(null)

    /**
     * How to say the link went over Bluetooth, or null for not now: only
     * on [road] 2 and only the first time. With the app in front
     * ([inFront]) a dialog, else a notification when one can show
     * ([canNotify]); when neither can, nothing is said yet, so the app
     * says it once in front. Marks it told.
     */
    fun next(ctx: Context, road: Int, inFront: Boolean, canNotify: Boolean): Way? {
        if (road != BluetoothRoadRule.ROAD_BLUETOOTH || Prefs.bluetoothTold(ctx)) return null
        val way = when {
            inFront -> Way.DIALOG
            canNotify -> Way.NOTIFICATION
            else -> return null
        }
        Prefs.setBluetoothTold(ctx)
        return way
    }

    /** Say, the first time only, that the link to the computer at [address] went over Bluetooth. */
    fun maybeTell(ctx: Context, address: String) {
        val app = ctx.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java)
        val way = synchronized(this) {
            next(app, RustCore.controlRoad(), inFront, canNotify(manager))
        } ?: return
        val name = HostStore.shownName(app, address)
        when (way) {
            Way.DIALOG -> Handler(Looper.getMainLooper()).post { pending.value = name }
            Way.NOTIFICATION -> manager?.let { notify(app, it, name) }
        }
    }

    /** The app came to the front: a road already on Bluetooth and never told is told now. */
    fun onFront(ctx: Context) {
        val address = RustCore.controlPeer()?.first ?: return
        if (BluetoothRoad.carries(address)) maybeTell(ctx, address)
    }

    private fun canNotify(manager: NotificationManager?): Boolean {
        if (manager == null || !manager.areNotificationsEnabled()) return false
        val channel = manager.getNotificationChannel(CHANNEL) ?: return true
        return channel.importance != NotificationManager.IMPORTANCE_NONE
    }

    private fun notify(app: Context, manager: NotificationManager, name: String) {
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, app.getString(R.string.channel_bluetooth), NotificationManager.IMPORTANCE_DEFAULT).apply {
                    description = app.getString(R.string.channel_bluetooth_desc)
                },
            )
        }
        val open = PendingIntent.getActivity(
            app,
            NOTIFICATION_ID,
            Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val setting = PendingIntent.getActivity(
            app,
            NOTIFICATION_ID + 1,
            Intent(app, MainActivity::class.java)
                .putExtra(EXTRA_OPEN_SETTING, true)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = app.getString(R.string.bluetooth_notice_text, name)
        val notification = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(app.getString(R.string.bluetooth_notice_title))
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, app.getString(R.string.bluetooth_notice_turn_off), setting).build())
            .setAutoCancel(true)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
    }
}
