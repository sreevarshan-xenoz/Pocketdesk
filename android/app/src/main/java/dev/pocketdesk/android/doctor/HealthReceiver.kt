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
            Intent.ACTION_MY_PACKAGE_REPLACED -> Unit
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
            } finally {
                pending.finish()
            }
        }.start()
    }

    companion object {
        private const val TAG = "HealthReceiver"
        private const val CHANNEL = "health"
        private const val NOTIFICATION_ID = 41_000

        /** Whether this boot runs another system build than the last one seen, which it records. */
        fun systemUpdated(ctx: Context): Boolean {
            val now = Build.FINGERPRINT ?: return false
            val last = Prefs.lastFingerprint(ctx)
            if (last == now) return false
            Prefs.setLastFingerprint(ctx, now)
            // The first boot seen is not an update.
            return last != null
        }

        /** Rebind, wait, check, and notify; on the receiver's own thread, within its time limit. */
        fun check(ctx: Context) {
            if (HealthCheck.rebindIfStopped(ctx)) Thread.sleep(HealthCheck.REBIND_WAIT_MS)
            val problems = HealthCheck.problems(ctx)
            if (problems.isEmpty()) {
                cancel(ctx)
            } else {
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
