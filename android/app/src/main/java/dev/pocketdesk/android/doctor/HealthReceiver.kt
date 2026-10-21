package dev.pocketdesk.android.doctor

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R

/**
 * After an update, says what stopped working (D1). The app updated
 * (`MY_PACKAGE_REPLACED`), or the phone started on a new system build
 * (`BOOT_COMPLETED` with another `Build.FINGERPRINT`): the listener is asked
 * to bind again, and what is still broken [HealthCheck.REBIND_WAIT_MS] later
 * posts one notification that opens Home. A restart on the same build does
 * nothing.
 */
class HealthReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext
        when (intent.action) {
            Intent.ACTION_MY_PACKAGE_REPLACED -> noteBuild(app)
            Intent.ACTION_BOOT_COMPLETED -> if (!systemUpdated(app)) return
            else -> return
        }
        if (!Prefs.healthAlerts(app)) return
        val pending = goAsync()
        Thread {
            try {
                check(app)
            } catch (e: RuntimeException) {
                Log.w(TAG, "health check after an update failed", e)
            } catch (_: InterruptedException) {
                // Given up with the process: nothing to tell.
            } finally {
                pending.finish()
            }
        }.start()
    }

    companion object {
        private const val TAG = "HealthReceiver"
        private const val CHANNEL = "health"
        // Below BackgroundFiles' 40_000 + n.
        private const val NOTIFICATION_ID = 39_000

        /** How long a boot is given to bind the listener: dexopt after an update makes it slow. */
        private const val LISTENER_WAIT_MS = 8_000L

        /** Whether this boot runs another system build than the last one seen, which it records. */
        fun systemUpdated(ctx: Context): Boolean {
            val updated = Health.systemUpdated(Prefs.lastFingerprint(ctx), Build.FINGERPRINT)
            noteBuild(ctx)
            return updated
        }

        /**
         * Record the system build now running: at boot, at every app start
         * and update (D1 review), so the first system update after installing
         * is told from a restart.
         */
        fun noteBuild(ctx: Context) {
            val now = Build.FINGERPRINT ?: return
            if (Prefs.lastFingerprint(ctx) != now) Prefs.setLastFingerprint(ctx, now)
        }

        /**
         * Rebind, wait for the listener, check, and notify; on the receiver's
         * own thread, within its time limit. Nothing without a saved computer:
         * Home, which would show and clear it, has no card then.
         */
        fun check(ctx: Context) {
            if (dev.pocketdesk.android.HostStore.hosts(ctx).isEmpty()) return
            // M1: a boot or an update may start the link's service, which the
            // listener, in the background, may not.
            runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(ctx) }
            // After a boot the listener may bind late (D1 review): wait for it,
            // up to [LISTENER_WAIT_MS], rather than judge it from one sample.
            if (HealthCheck.rebindIfStopped(ctx) || !ListenerState.connected) {
                val until = android.os.SystemClock.elapsedRealtime() + LISTENER_WAIT_MS
                while (!ListenerState.connected && android.os.SystemClock.elapsedRealtime() < until) Thread.sleep(250)
            }
            val problems = HealthCheck.problems(ctx)
            if (problems.isEmpty()) {
                cancel(ctx)
            } else if (Health.notifyAfterUpdate(problems, Prefs.healthSeen(ctx), Prefs.healthDismissed(ctx))) {
                // Only news (D1 review): a problem Home showed, or the owner put
                // off, before the update was not the update's doing.
                notify(ctx, problems.size)
            }
        }

        fun cancel(ctx: Context) {
            runCatching { ctx.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID) }
        }

        private fun notify(ctx: Context, count: Int) {
            val manager = ctx.getSystemService(NotificationManager::class.java) ?: return
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, ctx.getString(R.string.health_channel), NotificationManager.IMPORTANCE_DEFAULT),
            )
            val open = PendingIntent.getActivity(
                ctx,
                0,
                Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val notification = Notification.Builder(ctx, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_pocketdesk)
                .setContentTitle(ctx.getString(R.string.health_after_update))
                .setContentText(ctx.resources.getQuantityString(R.plurals.health_count, count, count))
                .setContentIntent(open)
                .setAutoCancel(true)
                .build()
            runCatching { manager.notify(NOTIFICATION_ID, notification) }
        }
    }
}
