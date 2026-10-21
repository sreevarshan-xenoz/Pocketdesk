package dev.pocketdesk.android.service

import android.app.ForegroundServiceStartNotAllowedException
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.doctor.HealthCheck
import dev.pocketdesk.android.doctor.StayLinked
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Stays linked with the screen off (M1). It does no work of its own: while
 * it runs, Android counts the app as in the foreground, so a maker's freezer
 * (ColorOS's freezes the whole app within 20 s of the screen going off) and
 * Doze's network cut leave the notification listener's tickers and links
 * alone. Measured on the OPPO: a probe holding one was never frozen in 15
 * minutes, and took a connection at once each minute.
 *
 * Type `connectedDevice`: it keeps the network link to the phone's paired
 * computer, which is what that type names. No wake lock: packets wake the
 * CPU, and the listener's callbacks wake it for notifications.
 */
class LinkKeeperService : Service() {
    companion object {
        private const val TAG = "LinkKeeper"
        private const val CHANNEL_ID = "pocketdesk-linked"
        private const val NOTIFICATION_ID = 41_000

        /** How long a start Android refused waits before the next try. */
        private const val RETRY_MS = 60_000L

        @Volatile
        private var running = false

        /** When Android last refused a start ([android.os.SystemClock.elapsedRealtime]). */
        @Volatile
        private var refusedAt = 0L

        /**
         * Start or stop the service as [StayLinked.wanted] says. Cheap enough
         * for the listener's 2 s tick: the reads are cached prefs and
         * [PhoneLink.target]'s kept answer. Android may refuse a start from the
         * background; the next start from the app takes over.
         */
        fun sync(ctx: Context) {
            val app = ctx.applicationContext
            val wanted = StayLinked.wanted(
                on = Prefs.stayLinked(app),
                features = HealthCheck.listenerFeatures(app),
                access = PhoneNotificationPrefs.accessGranted(app),
                paired = PhoneLink.target(app) != null,
            )
            if (wanted == running) return
            if (wanted) {
                val now = android.os.SystemClock.elapsedRealtime()
                if (refusedAt != 0L && now - refusedAt < RETRY_MS) return
                try {
                    app.startForegroundService(Intent(app, LinkKeeperService::class.java))
                } catch (e: Exception) {
                    // Android 12+ refuses some starts from the background
                    // (ForegroundServiceStartNotAllowedException); a later start
                    // from the app, a boot or an update takes over.
                    val refused = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                        e is ForegroundServiceStartNotAllowedException
                    Log.w(TAG, if (refused) "Android refused to start it from the background" else "Could not start it: $e")
                    refusedAt = now
                }
            } else {
                app.stopService(Intent(app, LinkKeeperService::class.java))
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        val notification = buildNotification()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        running = true
        refusedAt = 0L
        // A freeze seen before it ran is not news any more (M1).
        Prefs.setLastFreezeMs(this, null)
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_linked),
            NotificationManager.IMPORTANCE_MIN,
        )
        channel.setShowBadge(false)
        channel.description = getString(R.string.channel_linked_desc)
        (getSystemService(NOTIFICATION_SERVICE) as NotificationManager).createNotificationChannel(channel)
    }

    private fun buildNotification(): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val own = PhoneLink.target(this)
        val text = if (own != null) {
            getString(R.string.linked_to, HostStore.shownName(this, own.address))
        } else {
            getString(R.string.linked_to_computer)
        }
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
    }
}
