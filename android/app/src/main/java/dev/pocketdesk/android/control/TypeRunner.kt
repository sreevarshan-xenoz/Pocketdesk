package dev.pocketdesk.android.control

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import org.json.JSONObject

/** When Stop on the typing notification lets typing in again (M4). */
object TypingPrefs {
    private const val PREFS = "pocketdesk-typing"
    private const val KEY_STOPPED_UNTIL = "stopped_until"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Wall-clock milliseconds, so a restart keeps it. */
    fun stoppedUntil(ctx: Context): Long = prefs(ctx).getLong(KEY_STOPPED_UNTIL, 0L)

    fun stop(ctx: Context, nowMs: Long) = prefs(ctx).edit { putLong(KEY_STOPPED_UNTIL, nowMs + TypeRule.STOP_MS) }
}

/**
 * Typing from the computer's keyboard (M4), kept by the notification
 * listener: one thread in the process that waits for what the computer
 * types and does it through C8's Accessibility service, in order, never
 * into a password field and never while the phone is locked, at C8's rate.
 * It types only while its notification, with Stop, can be shown.
 */
object TypeRunner {
    private const val TAG = "TypeRunner"
    private const val CHANNEL = "typing"
    const val NOTIFICATION_ID = 42_000

    /** How long one wait for typing lasts. */
    private const val WAIT_MS = 5_000

    /** Which thread may type: a stopped one's last batch is not typed (M4 review). */
    @Volatile
    private var generation = 0
    private var running = false

    /** When something was last typed (elapsed realtime), 0 when the notice is not up. */
    @Volatile
    private var lastTypedMs = 0L

    /** Start or stop the thread as the switch and the service say: called on the listener's ticks. */
    @Synchronized
    fun sync(ctx: Context) {
        if (Prefs.typingFromComputer(ctx) && ControlService.instance != null) start(ctx) else stop(ctx)
    }

    @Synchronized
    fun start(ctx: Context) {
        if (running) return
        running = true
        val app = ctx.applicationContext
        // A notice a killed process left up says nothing true now.
        cancelNotice(app)
        val mine = ++generation
        Thread({ loop(app, mine) }, "pocketdesk-typing").apply {
            isDaemon = true
            start()
        }
    }

    @Synchronized
    fun stop(ctx: Context) {
        if (!running) return
        running = false
        generation++
        cancelNotice(ctx.applicationContext)
    }

    private fun loop(ctx: Context, mine: Int) {
        val rate = ControlPlan.Rate()
        while (generation == mine) {
            val batch = runCatching { RustCore.typeTake(WAIT_MS) }.getOrNull()
            // Stopped while waiting: this batch is not typed.
            if (generation != mine) break
            if (batch == null) {
                val last = lastTypedMs
                if (last != 0L && SystemClock.elapsedRealtime() - last >= TypeRule.NOTICE_MS) cancelNotice(ctx)
                continue
            }
            runCatching { perform(ctx, batch, rate, mine) }.onFailure { Log.w(TAG, "Typing failed", it) }
        }
    }

    private fun perform(ctx: Context, batch: RustCore.TypedBatch, rate: ControlPlan.Rate, mine: Int) {
        val own = PhoneLink.target(ctx)
        val decision = TypeRule.decide(
            on = Prefs.typingFromComputer(ctx),
            service = ControlService.instance != null,
            own = own != null && own.address == batch.host && own.controlPort == batch.port,
            stoppedUntilMs = TypingPrefs.stoppedUntil(ctx),
            nowMs = System.currentTimeMillis(),
        )
        if (decision != TypeRule.Decision.TYPE || own == null) {
            Log.i(TAG, "Not typed: $decision")
            return
        }
        // Typed only where the owner can see it is, and press Stop (M4
        // review: with the app's notifications off it typed unseen).
        if (!notice(ctx, own.address, password = false)) {
            Log.i(TAG, "Not typed: its notification cannot be shown")
            return
        }
        val service = ControlService.instance ?: return
        val refused = { notice(ctx, own.address, password = true); Unit }
        val events = org.json.JSONArray(batch.events)
        for (i in 0 until events.length()) {
            // Paced, never dropped (M4 review: a skipped Backspace changed the text).
            while (!rate.take(SystemClock.elapsedRealtime())) {
                if (generation != mine) return
                Thread.sleep(20)
            }
            val event: JSONObject = events.getJSONObject(i)
            when {
                event.has("text") -> service.typeBlind(event.getString("text"), refused)
                event.optString("key") == "backspace" -> service.backspaceBlind(refused)
                event.optString("key") == "enter" -> service.enterBlind(refused)
                event.has("global") -> service.globalBlind(event.getString("global"))
            }
        }
    }

    private fun manager(ctx: Context): NotificationManager? = ctx.getSystemService(NotificationManager::class.java)

    /**
     * Say that the computer at [address] is typing here, or that a password
     * field took nothing; false when Android will not show it. Posted again
     * when it was swiped away.
     */
    @Synchronized
    private fun notice(ctx: Context, address: String, password: Boolean): Boolean {
        val manager = manager(ctx) ?: return false
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.typing_channel), NotificationManager.IMPORTANCE_LOW),
        )
        val blocked = !manager.areNotificationsEnabled() ||
            manager.getNotificationChannel(CHANNEL)?.importance == NotificationManager.IMPORTANCE_NONE
        if (blocked) return false
        val up = lastTypedMs != 0L &&
            runCatching { manager.activeNotifications.any { it.id == NOTIFICATION_ID } }.getOrDefault(false)
        lastTypedMs = SystemClock.elapsedRealtime()
        if (up && !password) return true
        val stop = PendingIntent.getBroadcast(
            ctx,
            0,
            Intent(ctx, TypeStopReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val name = HostStore.shownName(ctx, address)
        val notification = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(ctx.getString(R.string.typing_now, name))
            .setContentText(ctx.getString(if (password) R.string.typing_password else R.string.typing_now_text))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(Notification.Action.Builder(null, ctx.getString(R.string.typing_stop), stop).build())
            .build()
        return runCatching { manager.notify(NOTIFICATION_ID, notification) }.isSuccess
    }

    @Synchronized
    private fun cancelNotice(ctx: Context) {
        lastTypedMs = 0L
        runCatching { manager(ctx)?.cancel(NOTIFICATION_ID) }
    }
}

/** Stop on the typing notification: no typing from the computer for ten minutes (M4). */
class TypeStopReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        TypingPrefs.stop(context, System.currentTimeMillis())
        runCatching { context.getSystemService(NotificationManager::class.java)?.cancel(TypeRunner.NOTIFICATION_ID) }
        android.widget.Toast.makeText(context, R.string.typing_stopped, android.widget.Toast.LENGTH_SHORT).show()
    }
}
