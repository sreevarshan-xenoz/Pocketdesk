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

        /** Whether the service is in the foreground now. */
        fun isRunning(): Boolean = running

        /** When Android last refused a start ([android.os.SystemClock.elapsedRealtime]). */
        @Volatile
        private var refusedAt = 0L

        /** Whether a refusal was logged as a warning; the next ones only at debug. */
        @Volatile
        private var warned = false

        /** The notification's words as last posted. */
        @Volatile
        private var postedText: String? = null

        private fun text(ctx: Context): String {
            val own = PhoneLink.target(ctx)
            return if (own != null) {
                ctx.getString(R.string.linked_to, HostStore.shownName(ctx, own.address))
            } else {
                ctx.getString(R.string.linked_to_computer)
            }
        }

        private fun build(ctx: Context): Notification {
            val open = PendingIntent.getActivity(
                ctx,
                0,
                Intent(ctx, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val words = text(ctx)
            postedText = words
            return Notification.Builder(ctx, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_stat_pocketdesk)
                .setContentTitle(ctx.getString(R.string.app_name))
                .setContentText(words)
                .setContentIntent(open)
                .setOngoing(true)
                .build()
        }

        private fun post(ctx: Context) {
            (ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).notify(NOTIFICATION_ID, build(ctx))
        }

        /** What [StayLinked.wanted] says for this phone now. */
        fun wanted(app: Context): Boolean = StayLinked.wanted(
            on = Prefs.stayLinked(app),
            features = HealthCheck.listenerFeatures(app),
            access = PhoneNotificationPrefs.accessGranted(app),
            paired = PhoneLink.target(app) != null,
        )

        /**
         * Start or stop the service as [StayLinked.wanted] says. Cheap enough
         * for the listener's 2 s tick: the reads are cached prefs and
         * [PhoneLink.target]'s kept answer. Android may refuse a start from the
         * background; the next start from the app takes over.
         */
        fun sync(ctx: Context) {
            val app = ctx.applicationContext
            val wanted = wanted(app)
            if (wanted && running) {
                // The computer renamed, or another became the phone's own:
                // the notification follows (M1 review).
                if (text(app) != postedText) runCatching { post(app) }
                return
            }
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
                    val line = if (refused) "Android refused to start it from the background" else "Could not start it: $e"
                    if (warned) Log.d(TAG, line) else Log.w(TAG, line)
                    warned = true
                    refusedAt = now
                }
            } else {
                app.stopService(Intent(app, LinkKeeperService::class.java))
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun wantedHere(): Boolean = wanted(applicationContext)

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        val notification = build(this)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (e: Exception) {
            // Android may refuse it here too (a start it allowed, then judged
            // again, or the type's prerequisites on 14+): stop rather than
            // crash the process, and let the next start try again (M1 review).
            Log.w(TAG, "Could not go to the foreground: $e")
            refusedAt = android.os.SystemClock.elapsedRealtime()
            stopSelf()
            return START_NOT_STICKY
        }
        // A start that raced the reason going (access taken away as the
        // listener stopped, the last feature turned off) ends here, once in
        // the foreground: a service started so must get there before it
        // stops, or Android ends the app (M1 review).
        if (!wantedHere()) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        refusedAt = 0L
        // Not sticky: a restart in a new process is not a start Android
        // excuses, and the listener's tick and the app start it again.
        return START_NOT_STICKY
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
}
