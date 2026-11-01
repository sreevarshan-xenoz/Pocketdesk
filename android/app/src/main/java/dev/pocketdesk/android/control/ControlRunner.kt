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
class ControlRunner(
    private val context: Context,
    /**
     * A session of app windows alone (Phone apps tier 2): there is no screen of the phone's
     * to control, so it never says control is on and drops every batch that is not for a window.
     */
    private val windowsOnly: Boolean = false,
    private val onState: (Boolean) -> Unit,
) {
    @Volatile
    private var running = true
    private val rate = ControlPlan.Rate()
    /**
     * What the computer was last told. Off to begin with, so a cast without
     * control says nothing (a desktop app from before C8 cannot read it).
     */
    private var said = false

    fun start() {
        Thread(::loop, "cast-control").start()
    }

    /** The cast ended: the thread stops at its next wait, and a finger down is lifted. */
    fun stop() {
        running = false
        ControlService.instance?.release()
        // Fingers held in app windows are lifted with the stream.
        dev.pocketdesk.android.cast.CastService.liftWindowFingers()
    }

    private fun loop() {
        while (running) {
            val service = ControlService.instance
            val on = !windowsOnly && Prefs.controlFromComputer(context) && service != null
            if (on != said && RustCore.castControlState(on) == CastPlan.QUEUED) {
                said = on
                onState(on)
                if (!on) ControlService.instance?.release()
            }
            // A press waiting to start (or to be lifted) needs a look soon.
            val wait = if (service?.waiting() == true) BUSY_WAIT_MS else IDLE_WAIT_MS
            val json = RustCore.castControlTake(wait)
            service?.tick()
            if (json == null) continue
            // Phone apps tier 2: a batch for an app window goes to that window,
            // whatever the phone's own screen control says (it has its own switch).
            val forWindows = windows(json)
            val target = dev.pocketdesk.android.windows.WindowRule.target(windowsOnly, forWindows, on)
            if (target != dev.pocketdesk.android.windows.WindowRule.Target.SCREEN || service == null) continue
            perform(service, json)
        }
    }

    /** Hand a batch for app windows to them; whether it was one. */
    private fun windows(json: String): Boolean {
        val batch = try {
            JSONObject(json)
        } catch (e: org.json.JSONException) {
            return false
        }
        val events = dev.pocketdesk.android.windows.WindowInput.forWindows(batch) ?: return false
        dev.pocketdesk.android.cast.CastService.windowInput(context, events)
        return true
    }

    private fun perform(service: ControlService, json: String) {
        val batch = try {
            JSONObject(json)
        } catch (e: org.json.JSONException) {
            return
        }
        val events = batch.optJSONArray("events") ?: return
        val now = SystemClock.uptimeMillis()
        // Adjacent text is typed at once: one round to the field, not one a character.
        var typed = StringBuilder()
        val flush = {
            if (typed.isNotEmpty()) {
                service.type(typed.toString())
                typed = StringBuilder()
            }
        }
        for (i in 0 until events.length()) {
            val event = events.optJSONObject(i) ?: continue
            // A press or release always goes: dropping one leaves a finger
            // down (C8 review); the rest is limited.
            val edge = event.optString("type") == "touch" && event.optString("action") != "move"
            if (!edge && !rate.take(now)) continue
            if (event.optString("type") == "text") {
                typed.append(event.optString("text"))
                continue
            }
            flush()
            when (event.optString("type")) {
                "touch" -> service.touch(event.optString("action"), event.optInt("x"), event.optInt("y"))
                "scroll" -> service.scroll(event.optInt("x"), event.optInt("y"), event.optInt("dy"))
                "global" -> service.global(event.optString("action"))
                "backspace" -> service.backspace()
                "enter" -> service.enter()
            }
        }
        flush()
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
