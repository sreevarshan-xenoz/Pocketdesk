package dev.pocketdesk.android.control

import android.content.ComponentName
import android.content.Context
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.cast.CastPlan
import org.json.JSONObject

/**
 * Control from the computer during a cast (C8): tells the computer whether
 * the phone takes control (the owner's setting and the service both on),
 * and hands what it sends to [ControlService]. It runs on its own thread,
 * waiting for each batch rather than polling, from the cast being shown to
 * its end.
 */
class ControlRunner(private val context: Context, private val onState: (Boolean) -> Unit) {
    @Volatile
    private var running = true
    private val rate = ControlPlan.Rate()
    /** What the computer was last told; null before the first word. */
    private var said: Boolean? = null

    fun start() {
        Thread(::loop, "cast-control").start()
    }

    /** The cast ended: the thread stops at its next wait, and a finger down is lifted. */
    fun stop() {
        running = false
        ControlService.instance?.release()
    }

    private fun loop() {
        while (running) {
            val service = ControlService.instance
            val on = Prefs.controlFromComputer(context) && service != null
            if (on != said && RustCore.castControlState(on) == CastPlan.QUEUED) {
                said = on
                onState(on)
                if (!on) ControlService.instance?.release()
            }
            // A press waiting to start (or to be lifted) needs a look soon.
            val wait = if (service?.waiting() == true) BUSY_WAIT_MS else IDLE_WAIT_MS
            val json = RustCore.castControlTake(wait)
            service?.tick()
            if (json == null || !on || service == null) continue
            perform(service, json)
        }
    }

    private fun perform(service: ControlService, json: String) {
        val batch = try {
            JSONObject(json)
        } catch (e: org.json.JSONException) {
            return
        }
        val events = batch.optJSONArray("events") ?: return
        val now = SystemClock.uptimeMillis()
        for (i in 0 until events.length()) {
            if (!rate.take(now)) {
                Log.w(TAG, "too much control at once; the rest of this batch is dropped")
                break
            }
            val event = events.optJSONObject(i) ?: continue
            when (event.optString("type")) {
                "touch" -> service.touch(event.optString("action"), event.optInt("x"), event.optInt("y"))
                "scroll" -> service.scroll(event.optInt("x"), event.optInt("y"), event.optInt("dy"))
                "global" -> service.global(event.optString("action"))
                "text" -> service.type(event.optString("text"))
                "backspace" -> service.backspace()
                "enter" -> service.enter()
            }
        }
        // For measuring the delay against the computer's clock (C8).
        Log.d(TAG, "batch sent=${batch.optLong("sent")} taken=${System.currentTimeMillis()}")
    }

    companion object {
        private const val TAG = "ControlRunner"
        private const val IDLE_WAIT_MS = 250
        private const val BUSY_WAIT_MS = 20

        /** Whether the owner turned the service on in Android's Accessibility settings. */
        fun serviceEnabled(context: Context): Boolean {
            val enabled = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            val ours = ComponentName(context, ControlService::class.java)
            return enabled.split(':').any { ComponentName.unflattenFromString(it) == ours }
        }
    }
}
