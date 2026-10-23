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
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.compat.Android17
import dev.pocketdesk.android.notify.PhoneLink

/**
 * The phone rings because its computer asked (N1): a notification on its
 * own channel with the alarm sound, insistent (repeating until stopped), on
 * the lock screen in full, with Stop, gone after [RING_MS]. An alarm is
 * heard in Do Not Disturb when alarms are allowed, Android's default, and a
 * notification's sound plays with the app in the background, so no
 * foreground service is needed. The alarm stream is at its loudest while it
 * rings, and put back after, before Android 17; from 17 it rings at the
 * owner's alarm volume, since a volume set from the background is ignored
 * there (N4).
 */
object PhoneRinger {
    private const val TAG = "PhoneRinger"
    /**
     * The ring's channel. A channel's vibration cannot change once made, so
     * P11's has none, the switch driving it ([vibrate]); the first one,
     * which vibrated, is retired (P11 review).
     */
    private const val CHANNEL = "ring_from_computer_2"
    private const val OLD_CHANNEL = "ring_from_computer"
    private const val NOTIFICATION_ID = 7301
    const val RING_MS = 30_000L

    private val main = Handler(Looper.getMainLooper())

    /** The alarm volume before a ring raised it, while one rings. */
    @Volatile
    private var volumeBefore: Int? = null

    /**
     * The tone played straight on the alarm stream when the notification
     * cannot sound (N1 review: notifications off, or the channel muted).
     */
    @Volatile
    private var tone: android.media.Ringtone? = null

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
            .onFailure { Log.w(TAG, "could not post the ring", it) }
        // A notification that cannot sound rings nothing: the alarm tone
        // plays straight on the alarm stream then.
        if (!notificationSounds(manager)) {
            // Android 17 silences it unless the app is open (N4).
            Log.i(TAG, "the ring's notification cannot sound; playing the tone itself")
            playTone(app)
        }
        main.removeCallbacksAndMessages(null)
        // P11: it shakes and flashes while it sounds, each as Settings say;
        // a ring before it with a switch on since turned off stops shaking
        // or flashing (P11 review).
        if (Prefs.ringVibrates(app)) vibrate(app) else runCatching { vibrator(app)?.cancel() }
        val startedAt = SystemClock.elapsedRealtime()
        // On the main thread, as its stop is (P11 review: a tick's thread
        // could set the torch between a stop's two steps).
        main.post { if (Prefs.ringFlashes(app)) flash(app, startedAt) else torchOff(app) }
        main.postDelayed({ stopSound(app) }, RING_MS)
    }

    /** The vibrator, as each Android names it. */
    private fun vibrator(ctx: Context): android.os.Vibrator? =
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            ctx.getSystemService(android.os.VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            ctx.getSystemService(android.os.Vibrator::class.java)
        }

    /** P11: the ring's pattern, repeated until it stops, as an alarm's. */
    private fun vibrate(ctx: Context) {
        runCatching {
            val vibrator = vibrator(ctx)?.takeIf { it.hasVibrator() } ?: return
            val effect = android.os.VibrationEffect.createWaveform(RingPattern.VIBRATION, 0)
            if (android.os.Build.VERSION.SDK_INT >= 33) {
                vibrator.vibrate(
                    effect,
                    android.os.VibrationAttributes.createForUsage(android.os.VibrationAttributes.USAGE_ALARM),
                )
            } else {
                @Suppress("DEPRECATION")
                vibrator.vibrate(
                    effect,
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).build(),
                )
            }
        }.onFailure { Log.w(TAG, "could not vibrate the ring", it) }
    }

    /** The camera with a torch, if any. */
    private fun torchCamera(camera: android.hardware.camera2.CameraManager): String? =
        runCatching {
            camera.cameraIdList.firstOrNull { id ->
                camera.getCameraCharacteristics(id)
                    .get(android.hardware.camera2.CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
            }
        }.getOrNull()

    /** Whether this ring turned the torch on, to turn it off after. */
    @Volatile
    private var torchId: String? = null

    /**
     * P11: the torch blinks while it rings, from [startedAt]; never one a
     * camera holds (setTorchMode throws then, and the ring goes on).
     */
    private fun flash(ctx: Context, startedAt: Long) {
        val camera = ctx.getSystemService(android.hardware.camera2.CameraManager::class.java) ?: return
        val id = torchId ?: torchCamera(camera) ?: return
        val elapsed = SystemClock.elapsedRealtime() - startedAt
        if (elapsed >= RING_MS) {
            torchOff(ctx)
            return
        }
        val ok = runCatching { camera.setTorchMode(id, RingPattern.torchOn(elapsed)) }
            .onFailure { Log.w(TAG, "could not flash the torch", it) }
            .isSuccess
        if (!ok) {
            // A camera holds it now: tried again in a second, while it
            // rings (P11 review: it never came back).
            main.postDelayed({ flash(ctx, startedAt) }, TORCH_RETRY_MS)
            return
        }
        torchId = id
        main.postDelayed({ flash(ctx, startedAt) }, RingPattern.nextTorchChangeMs(elapsed))
    }

    private fun torchOff(ctx: Context) {
        val id = torchId ?: return
        torchId = null
        runCatching {
            ctx.getSystemService(android.hardware.camera2.CameraManager::class.java)?.setTorchMode(id, false)
        }
    }

    fun stop(ctx: Context) {
        val app = ctx.applicationContext
        app.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
        main.removeCallbacksAndMessages(null)
        stopSound(app)
    }

    /** Whether the ring's notification can be heard: notifications on, the channel loud. */
    private fun notificationSounds(manager: NotificationManager): Boolean {
        if (!manager.areNotificationsEnabled()) return false
        val channel = manager.getNotificationChannel(CHANNEL) ?: return false
        return channel.importance >= NotificationManager.IMPORTANCE_DEFAULT && channel.sound != null
    }

    private fun alarmSound(ctx: Context): android.net.Uri =
        RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_RINGTONE)
            ?: android.provider.Settings.System.DEFAULT_ALARM_ALERT_URI

    private fun playTone(ctx: Context) {
        runCatching {
            tone?.stop()
            val ringtone = RingtoneManager.getRingtone(ctx, alarmSound(ctx)) ?: return
            ringtone.audioAttributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build()
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) ringtone.isLooping = true
            ringtone.play()
            tone = ringtone
        }.onFailure { Log.w(TAG, "could not play the ring's tone", it) }
    }

    /** The tone stopped, if one plays, and the alarm volume put back. */
    private fun stopSound(ctx: Context) {
        runCatching { tone?.stop() }
        tone = null
        // P11: the shaking and the torch with it.
        runCatching { vibrator(ctx)?.cancel() }
        torchOff(ctx)
        restoreVolume(ctx)
    }

    private const val TORCH_RETRY_MS = 1_000L

    private fun ensureChannel(ctx: Context, manager: NotificationManager) {
        if (manager.getNotificationChannel(OLD_CHANNEL) != null) {
            runCatching { manager.deleteNotificationChannel(OLD_CHANNEL) }
        }
        if (manager.getNotificationChannel(CHANNEL) != null) return
        val sound = alarmSound(ctx)
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
            // Shaken by the switch, not the channel (P11 review).
            enableVibration(false)
            // Heard through Do Not Disturb where the owner lets the app do so.
            setBypassDnd(true)
            lockscreenVisibility = Notification.VISIBILITY_PUBLIC
        }
        manager.createNotificationChannel(channel)
    }

    /** Where the volume before a ring is kept, so a process killed mid-ring still puts it back. */
    private fun saved(ctx: Context) = ctx.getSharedPreferences("pocketdesk-ring", Context.MODE_PRIVATE)

    private const val KEY_VOLUME_BEFORE = "alarm_volume_before"

    /** Whether a ring sounds now (in this process). */
    val ringing: Boolean
        get() = volumeBefore != null || tone != null

    private fun loudest(ctx: Context) {
        // Android 17: ignored from the background, and a raise made while
        // the app was open might never be put back.
        if (!Android17.ringRaisesVolume(android.os.Build.VERSION.SDK_INT)) return
        runCatching {
            val audio = ctx.getSystemService(AudioManager::class.java)
            if (volumeBefore == null) {
                val before = audio.getStreamVolume(AudioManager.STREAM_ALARM)
                volumeBefore = before
                // A volume saved by a ring cut short stays the one to restore.
                if (!saved(ctx).contains(KEY_VOLUME_BEFORE)) {
                    // On disk before the volume goes up.
                    saved(ctx).edit(commit = true) { putInt(KEY_VOLUME_BEFORE, before) }
                }
            }
            audio.setStreamVolume(AudioManager.STREAM_ALARM, audio.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0)
        }
    }

    private fun restoreVolume(ctx: Context) {
        val kept = saved(ctx).getInt(KEY_VOLUME_BEFORE, -1).takeIf { it >= 0 }
        val before = kept ?: volumeBefore ?: return
        volumeBefore = null
        val back = runCatching {
            val audio = ctx.getSystemService(AudioManager::class.java)
            audio.setStreamVolume(AudioManager.STREAM_ALARM, before, 0)
            audio.getStreamVolume(AudioManager.STREAM_ALARM) == before
        }.getOrDefault(false)
        // Android 17 ignores a volume set from the background (N4 review): a
        // volume a ring left raised (before an upgrade to 17, say) stays
        // saved until it is really put back, at the latest when the app is
        // next opened.
        if (back) saved(ctx).edit { remove(KEY_VOLUME_BEFORE) }
    }

    /**
     * Put back an alarm volume a ring left raised when its process died
     * mid-ring (N1 review); nothing while one rings here.
     */
    fun restoreAfterDeath(ctx: Context) {
        if (!ringing && saved(ctx).contains(KEY_VOLUME_BEFORE)) restoreVolume(ctx.applicationContext)
    }
}

/** Stop on the ringing notification, or the notification swiped away. */
class StopRingReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        PhoneRinger.stop(context)
    }
}

/**
 * Whether a session or the remote is on screen with its [SirenWatcher]
 * (dev.pocketdesk.android.ui), which then takes the ring latch itself; the
 * background tick takes it otherwise (N1 review: a remote, which had no
 * watcher, left a ring in the latch to sound when it closed).
 */
object RingOwner {
    @Volatile
    private var watchers = 0

    fun watching() {
        synchronized(this) { watchers++ }
    }

    fun left() {
        synchronized(this) { watchers = (watchers - 1).coerceAtLeast(0) }
    }

    val watched: Boolean
        get() = watchers > 0
}

/**
 * The notification listener's tick for N1: keep the link to the phone's
 * computer while "Ring from the computer" is on, and take the bridge's ring
 * latch outside a session ([RingRule]).
 */
class BackgroundRing(private val ctx: Context) {
    // Under the computer's minute, so tick timing never drops a ring the
    // computer let through (N1 review).
    private val rule = RingRule(everyMs = 45_000L)
    private var lastConnectMs = 0L

    fun tick() {
        PhoneRinger.restoreAfterDeath(ctx)
        val on = Prefs.ringFromComputer(ctx)
        val now = SystemClock.elapsedRealtime()
        if (on) {
            val own = PhoneLink.target(ctx)
            if (own != null && RustCore.controlPeer() == null && now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        // A session or the remote on screen takes the latch itself.
        val inSession = RingOwner.watched
        val asked = !inSession && RustCore.checkSiren()
        when (rule.decide(asked, on, inSession, now)) {
            // Named by the computer whose link carried it (N1 review).
            RingRule.Act.RING ->
                PhoneRinger.ring(ctx, RustCore.controlPeer()?.let { HostStore.name(ctx, it.first) })
            RingRule.Act.TAKE_AND_DROP -> Log.i("BackgroundRing", "a ring from the computer, not rung (switch off, or within a minute)")
            RingRule.Act.NOTHING -> {}
        }
    }

    private companion object {
        // As the other background features retry: a computer away costs a dial a minute.
        const val RECONNECT_MS = 60_000L
    }
}
