package dev.pocketdesk.android.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore

/**
 * Keeps the process alive while a remote-desktop streaming session is up
 * (Tier 1 #5). Started by [dev.pocketdesk.android.ui.RemoteScreen] on entry,
 * stopped on exit; the notification offers a Disconnect action that routes
 * back through MainActivity.
 *
 * FGS type choice: `specialUse` with a remote-desktop subtype. The client
 * renders a decoded stream — it captures no video on-device — so
 * `mediaProjection`'s consent dialog would be semantically wrong and scary;
 * `dataSync`/`connectedDevice` do not match Play policy. The justification
 * lives in the manifest's PROPERTY_SPECIAL_USE_FGS_SUBTYPE property. The
 * `microphone` type rides along once RECORD_AUDIO is granted (R4 E2 mic
 * share) — Android 14 requires it for off-foreground mic access.
 */
class SessionForegroundService : Service() {
    companion object {
        const val ACTION_START = "dev.pocketdesk.android.action.START_SESSION"
        const val ACTION_STOP = "dev.pocketdesk.android.action.STOP_SESSION"
        const val EXTRA_ADDRESS = "address"
        const val EXTRA_PORT = "port"
        const val EXTRA_CONTROL_PORT = "controlPort"

        private const val CHANNEL_ID = "pocketdesk-session"
        private const val NOTIFICATION_ID = 1

        /** Safety bound for the session wake lock: 6 h, extended on restart. */
        private const val WAKE_LOCK_TIMEOUT_MS = 6L * 60 * 60 * 1000
    }

    private var sessionAddress = "remote host"
    private var sessionControlPort = 1716

    override fun onDestroy() {
        if (wakeLock.isHeld) wakeLock.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /// Partial wake lock (Tier-3 #13 Doze strategy): an unattended screen-off
    /// session must keep draining frames and answering the QUIC keepalive —
    /// Doze would otherwise suspend the CPU and let the idle timeout kill a
    /// perfectly healthy link. Bounded timeout so a stuck service can never
    /// hold the device awake forever; re-acquired (extended) on every start.
    private val wakeLock: PowerManager.WakeLock by lazy {
        (getSystemService(POWER_SERVICE) as PowerManager)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "pocketdesk:session")
            .apply { setReferenceCounted(false) }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        wakeLock.acquire(WAKE_LOCK_TIMEOUT_MS)
        createChannel()
        sessionAddress = intent?.getStringExtra(EXTRA_ADDRESS) ?: sessionAddress
        intent?.getIntExtra(EXTRA_CONTROL_PORT, 0)?.takeIf { it > 0 }?.let {
            sessionControlPort = it
        }
        val notification = buildNotification(sessionAddress)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            // R4 E2: Android 14+ only lets a backgrounded/PiP session touch the
            // microphone while the FGS carries the microphone type — and
            // passing that type without RECORD_AUDIO granted throws, so it is
            // OR-ed in conditionally. RemoteScreen re-issues the start intent
            // right after the permission is granted to upgrade a live service.
            var type = ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
                android.content.pm.PackageManager.PERMISSION_GRANTED
            ) {
                type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            }
            startForeground(NOTIFICATION_ID, notification, type)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        return START_NOT_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // UI swiped away: do not leave a live streaming session running.
        Thread { runCatching { RustCore.stopStreaming() } }.start()
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_session),
            NotificationManager.IMPORTANCE_LOW,
        )
        channel.setShowBadge(false)
        channel.description = getString(R.string.channel_session_desc)
        notificationManager().createNotificationChannel(channel)
    }

    private fun buildNotification(address: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val disconnect = PendingIntent.getActivity(
            this,
            1,
            Intent(this, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_DISCONNECT, true)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        // Tier-3 #15 carry-over from Tier 1 #5: lock the *desktop* straight
        // from the notification — MainActivity routes it to the privacy
        // plugin without disturbing the open session.
        val lockDesktop = PendingIntent.getActivity(
            this,
            2,
            Intent(this, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_LOCK_DESKTOP, true)
                .putExtra(EXTRA_ADDRESS, sessionAddress)
                .putExtra(EXTRA_CONTROL_PORT, sessionControlPort)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(getString(R.string.notif_streaming, address))
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, R.drawable.ic_notif_disconnect),
                    getString(R.string.disconnect),
                    disconnect,
                ).build(),
            )
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, R.drawable.ic_notif_lock),
                    getString(R.string.lock_desktop),
                    lockDesktop,
                ).build(),
            )
            .build()
    }

    private fun notificationManager(): NotificationManager =
        getSystemService(NOTIFICATION_SERVICE) as NotificationManager
}
