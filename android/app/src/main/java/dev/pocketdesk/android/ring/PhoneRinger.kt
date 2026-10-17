package dev.pocketdesk.android.ring

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.RingtoneManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink

/**
 * The phone rings because its computer asked (N1): a notification on its
 * own channel with the alarm sound, insistent (repeating until stopped), on
 * the lock screen in full, with Stop, gone after [RING_MS]. An alarm is
 * heard in Do Not Disturb when alarms are allowed, Android's default, and a
 * notification's sound plays with the app in the background, so no
 * foreground service is needed. The alarm stream is at its loudest while it
 * rings, and put back after.
 */
object PhoneRinger {
    private const val TAG = "PhoneRinger"
    private const val CHANNEL = "ring_from_computer"
    private const val NOTIFICATION_ID = 7301
    const val RING_MS = 30_000L

    private val main = Handler(Looper.getMainLooper())

    /** The alarm volume before a ring raised it, while one rings. */
    @Volatile
    private var volumeBefore: Int? = null

    /** Ring, naming [computer] (U3) when it has a name. */
    fun ring(ctx: Context, computer: String? = null) {
        val app = ctx.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java)
        ensureChannel(app, manager)
        loudest(app)
        val stop = PendingIntent.getBroadcast(
            app,
            0,
            Intent(app, StopRingReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val open = PendingIntent.getActivity(
            app,
            0,
            Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(
                if (computer != null) {
                    app.getString(R.string.ring_title_named, computer)
                } else {
                    app.getString(R.string.ring_title)
                },
            )
            .setContentText(app.getString(R.string.ring_text))
            .setCategory(Notification.CATEGORY_ALARM)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setFullScreenIntent(open, true)
            .setContentIntent(open)
            .setDeleteIntent(stop)
            .setTimeoutAfter(RING_MS)
            .addAction(Notification.Action.Builder(null, app.getString(R.string.ring_stop), stop).build())
            .build()
        // Repeat the sound until it is stopped, dismissed or times out.
        notification.flags = notification.flags or Notification.FLAG_INSISTENT
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
            .onFailure { Log.w(TAG, "could not ring", it) }
        main.removeCallbacksAndMessages(null)
        main.postDelayed({ restoreVolume(app) }, RING_MS)
    }

    fun stop(ctx: Context) {
        val app = ctx.applicationContext
        app.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
        main.removeCallbacksAndMessages(null)
        restoreVolume(app)
    }

    private fun ensureChannel(ctx: Context, manager: NotificationManager) {
        if (manager.getNotificationChannel(CHANNEL) != null) return
        val sound = RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_RINGTONE)
        val channel = NotificationChannel(
            CHANNEL,
            ctx.getString(R.string.ring_channel),
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = ctx.getString(R.string.ring_channel_description)
            setSound(
                sound,
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_ALARM)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                    .build(),
            )
            enableVibration(true)
            // Heard through Do Not Disturb where the owner lets the app do so.
            setBypassDnd(true)
            lockscreenVisibility = Notification.VISIBILITY_PUBLIC
        }
        manager.createNotificationChannel(channel)
    }

    private fun loudest(ctx: Context) {
        runCatching {
            val audio = ctx.getSystemService(AudioManager::class.java)
            if (volumeBefore == null) volumeBefore = audio.getStreamVolume(AudioManager.STREAM_ALARM)
            audio.setStreamVolume(AudioManager.STREAM_ALARM, audio.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0)
        }
    }

    private fun restoreVolume(ctx: Context) {
        val before = volumeBefore ?: return
        volumeBefore = null
        runCatching {
            ctx.getSystemService(AudioManager::class.java).setStreamVolume(AudioManager.STREAM_ALARM, before, 0)
        }
    }
}

/** Stop on the ringing notification, or the notification swiped away. */
class StopRingReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        PhoneRinger.stop(context)
    }
}

/**
 * The notification listener's tick for N1: keep the link to the phone's
 * computer while "Ring from the computer" is on, and take the bridge's ring
 * latch outside a session ([RingRule]).
 */
class BackgroundRing(private val ctx: Context) {
    private val rule = RingRule()
    private var lastConnectMs = 0L

    fun tick() {
        val on = Prefs.ringFromComputer(ctx)
        val now = SystemClock.elapsedRealtime()
        if (on) {
            val own = PhoneLink.target(ctx)
            if (own != null && RustCore.controlPeer() == null && now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        val inSession = RustCore.isStreamingActive
        // The latch is taken only outside a session, whose watcher takes it.
        val asked = !inSession && RustCore.checkSiren()
        when (rule.decide(asked, on, inSession, now)) {
            RingRule.Act.RING ->
                PhoneRinger.ring(ctx, PhoneLink.target(ctx)?.let { HostStore.name(ctx, it.address) })
            RingRule.Act.TAKE_AND_DROP -> Log.i("BackgroundRing", "a ring from the computer, not rung (switch off, or within a minute)")
            RingRule.Act.NOTHING -> {}
        }
    }

    private companion object {
        const val RECONNECT_MS = 30_000L
    }
}
