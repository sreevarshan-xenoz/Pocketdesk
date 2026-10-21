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
 * Typing from the computer's keyboard (M4), on the notification listener: a
 * thread that waits for what the computer types and does it through C8's
 * Accessibility service, never into a password field and never while the
 * phone is locked, at C8's rate. While it types, an ongoing notification
 * says so, with Stop.
 */
class TypeRunner(private val ctx: Context) {
    @Volatile
    private var running = false
    private var thread: Thread? = null
    private val rate = ControlPlan.Rate()

    /** When something was last typed (elapsed realtime), 0 when the notice is not up. */
    private var lastTypedMs = 0L

    fun start() {
        if (running) return
        running = true
        thread = Thread({ loop() }, "pocketdesk-typing").apply {
            isDaemon = true
            start()
        }
    }

    fun stop() {
        running = false
        thread?.interrupt()
        thread = null
        cancelNotice()
    }

    private fun loop() {
        while (running) {
            val batch = runCatching { RustCore.typeTake(WAIT_MS) }.getOrNull()
            if (batch == null) {
                if (lastTypedMs != 0L && SystemClock.elapsedRealtime() - lastTypedMs >= TypeRule.NOTICE_MS) cancelNotice()
                continue
            }
            runCatching { perform(batch) }.onFailure { Log.w(TAG, "Typing failed", it) }
        }
    }

    private fun perform(batch: RustCore.TypedBatch) {
        val own = PhoneLink.target(ctx)
        val decision = TypeRule.decide(
            on = Prefs.typingFromComputer(ctx),
            service = ControlService.instance != null,
            own = own != null && own.address == batch.host && own.controlPort == batch.port,
            stoppedUntilMs = TypingPrefs.stoppedUntil(ctx),
            nowMs = System.currentTimeMillis(),
        )
        if (decision != TypeRule.Decision.TYPE) {
            Log.i(TAG, "Not typed: $decision")
            return
        }
        val service = ControlService.instance ?: return
        val refused = { notice(own!!.address, password = true) }
        val events = org.json.JSONArray(batch.events)
        for (i in 0 until events.length()) {
            if (!rate.take(SystemClock.elapsedRealtime())) continue
            val event: JSONObject = events.getJSONObject(i)
            when {
                event.has("text") -> service.typeBlind(event.getString("text"), refused)
                event.optString("key") == "backspace" -> service.backspaceBlind(refused)
                event.optString("key") == "enter" -> service.enterBlind(refused)
                event.has("global") -> service.global(event.getString("global"))
            }
        }
        notice(own!!.address, password = false)
    }

    private fun manager(): NotificationManager? = ctx.getSystemService(NotificationManager::class.java)

    /** Say that the computer at [address] is typing here, or that a password field took nothing. */
    private fun notice(address: String, password: Boolean) {
        val first = lastTypedMs == 0L
        lastTypedMs = SystemClock.elapsedRealtime()
        if (!first && !password) return
        val manager = manager() ?: return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.typing_channel), NotificationManager.IMPORTANCE_LOW),
        )
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
            .addAction(Notification.Action.Builder(null, ctx.getString(R.string.typing_stop), stop).build())
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
    }

    private fun cancelNotice() {
        lastTypedMs = 0L
        runCatching { manager()?.cancel(NOTIFICATION_ID) }
    }

    companion object {
        private const val TAG = "TypeRunner"
        private const val CHANNEL = "typing"
        const val NOTIFICATION_ID = 42_000

        /** How long one wait for typing lasts. */
        private const val WAIT_MS = 5_000
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
