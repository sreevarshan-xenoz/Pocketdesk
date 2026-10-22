package dev.pocketdesk.android.automation

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.shortcuts.ShortcutPlan
import dev.pocketdesk.android.shortcuts.WidgetAsk
import dev.pocketdesk.android.ui.Deck
import dev.pocketdesk.android.ui.TaskRun
import org.json.JSONObject
import java.util.UUID

/**
 * O1: an automation app (Tasker, MacroDroid, Samsung's routines) starts one
 * of the computer's tasks or presses one of its deck buttons by name, as
 * the Tasks sheet and the deck do, with [AutomationRule] deciding whether
 * it may. How it went is told back as the events of `docs/phone-automation.md`
 * (`TASK_FINISHED`, `DECK_PRESSED`) when "Tell other apps what happens" is on.
 */
class AutomationReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val runsTask = when (intent.action) {
            ACTION_RUN_TASK -> true
            ACTION_PRESS_DECK -> false
            else -> return
        }
        val app = context.applicationContext
        // The switch first, and quietly (O1 review): with it off, nothing an
        // app sends is read, so an extra Pocketdesk cannot unpack never
        // reaches the process.
        if (!Prefs.othersRunTasks(app)) {
            Log.i(TAG, "Not sent: Let other apps run the computer's tasks is off")
            return
        }
        val (rawAddress, rawName) =
            runCatching { intent.getStringExtra(PhoneEvents.EXTRA_ADDRESS) to intent.getStringExtra(EXTRA_NAME) }
                .getOrDefault(null to null)
        val address = rawAddress?.trim().orEmpty()
        val host = ShortcutPlan.savedHost(address, HostStore.hosts(app))
        val name = AutomationRule.name(rawName)
        val refusal = admit(host != null, name)
        if (refusal != null || host == null || name == null) {
            Log.i(TAG, "Not sent (${intent.action}): ${refusal?.reason}")
            val told = mapOf("state" to "refused", "reason" to refusal?.reason)
            // An unsaved address is the caller's, of any length: told cut short.
            val toldAddress = address.take(MAX_TOLD_ADDRESS)
            if (runsTask) tellTask(app, toldAddress, name.orEmpty(), told) else tellPress(app, toldAddress, name.orEmpty(), told)
            return
        }
        if (runsTask) runTask(app, host, name) else press(app, host, name)
    }

    private fun runTask(app: Context, host: HostStore.Host, name: String) {
        WidgetAsk.bounded(this, "automation-task", onTimeout = { Log.i(TAG, "The computer has not answered yet") }) {
            val listed = WidgetAsk.ask(app) { RustCore.listTasks(host.address, host.controlPort) }
            val body = listed.mapCatching { JSONObject(it) }.getOrNull()
            val error = body?.optString("error")?.takeIf { it.isNotBlank() }
            if (body == null || error != null) {
                Log.i(TAG, "No task list: ${error ?: listed.exceptionOrNull()?.message}")
                TaskFollow.tellFinished(app, host.address, name, TaskRun.Status.NotSent(error.orEmpty()))
                return@bounded
            }
            val tasks = body.optJSONArray("tasks")
            val task = (0 until (tasks?.length() ?: 0)).mapNotNull { tasks?.optJSONObject(it) }
                .firstOrNull { it.optString("name") == name }
            if (task == null) {
                Log.i(TAG, "The computer has no task by that name")
                TaskFollow.tellFinished(app, host.address, name, TaskRun.Status.Unknown)
                return@bounded
            }
            val asks = task.optBoolean("ask", true)
            // A start and its result ride the control link (1c review). It is
            // brought up only to the phone's own computer, whose link the
            // background features keep anyway, and never taken from another
            // (O1 review: a link to a second computer would have pointed them
            // all there).
            val peer = runCatching { RustCore.controlPeer() }.getOrNull()
            val own = PhoneLink.target(app)
            val link = AutomationRule.link(
                linked = peer,
                wanted = host.address to host.controlPort,
                own = own?.let { it.address to it.controlPort },
            )
            if (link == AutomationRule.Link.LINKED_ELSEWHERE || link == AutomationRule.Link.NOT_LINKED) {
                Log.i(TAG, "Not started: ${link.reason}")
                tellTask(app, host.address, name, mapOf("state" to "not_sent", "reason" to link.reason))
                return@bounded
            }
            if (link == AutomationRule.Link.CONNECT) {
                runCatching { RustCore.connectToPeer(host.address, host.controlPort, HostStore.wanIdentity(app, host.address)) }
                    .onFailure { Log.i(TAG, "No link: ${it.message}") }
            }
            val id = UUID.randomUUID().toString()
            val since = SystemClock.elapsedRealtime()
            val started = WidgetAsk.ask(app) { RustCore.runTask(host.address, host.controlPort, name, id).map { "" } }
            started.exceptionOrNull()?.let {
                Log.i(TAG, "Not started: ${it.message}")
                TaskFollow.tellFinished(app, host.address, name, TaskRun.Status.NotSent(it.message.orEmpty()))
                return@bounded
            }
            Log.i(TAG, "Task started for another app")
            // Followed only for whom it is told to.
            if (Prefs.tellOtherApps(app)) TaskFollow.toTheEnd(app, host.address, name, id, asks, since)
        }
    }

    private fun press(app: Context, host: HostStore.Host, label: String) {
        WidgetAsk.bounded(this, "automation-press", onTimeout = { Log.i(TAG, "The computer has not answered yet") }) {
            val listed = WidgetAsk.ask(app) { RustCore.listDeck(host.address, host.controlPort) }
            val buttons = listed.mapCatching { Deck.parse(it) }
            val button = buttons.getOrNull()?.let { AutomationRule.button(it, label) }
            if (button == null) {
                val missing = buttons.isSuccess
                Log.i(TAG, if (missing) "The deck has no button by that label" else "No deck: ${listed.exceptionOrNull()?.message}")
                tellPress(app, host.address, label, if (missing) mapOf("state" to "refused", "reason" to "unknown") else AutomationRule.pressed(null))
                return@bounded
            }
            val answer = WidgetAsk.ask(app) { RustCore.pressDeck(host.address, host.controlPort, button.index, button.label) }
            val outcome = answer.mapCatching { Deck.outcome(it) }.getOrNull()
            Log.i(TAG, "Deck pressed for another app: ${AutomationRule.pressed(outcome)["state"]}")
            tellPress(app, host.address, label, AutomationRule.pressed(outcome))
        }
    }

    companion object {
        private const val TAG = "Automation"

        /** Start a task: `ADDRESS` names the saved computer, `NAME` the task. */
        const val ACTION_RUN_TASK = "dev.pocketdesk.android.action.RUN_TASK"

        /** Press a deck button: `ADDRESS` names the saved computer, `NAME` the button's label. */
        const val ACTION_PRESS_DECK = "dev.pocketdesk.android.action.PRESS_DECK"

        /** The task's name or the button's label, exactly as the computer lists it. */
        const val EXTRA_NAME = PhoneEvents.EXTRA_PREFIX + "NAME"

        /** The most of an unsaved address an event repeats. */
        private const val MAX_TOLD_ADDRESS = 64

        /** When the asks of the last minute came (elapsed realtime), from all apps. */
        private val recent = ArrayList<Long>()

        private fun admit(saved: Boolean, name: String?): AutomationRule.Refusal? = synchronized(recent) {
            val now = SystemClock.elapsedRealtime()
            val kept = AutomationRule.kept(recent, now)
            recent.clear()
            recent.addAll(kept)
            AutomationRule.refusal(saved, name, recent, now).also { if (it == null) recent.add(now) }
        }

        private fun tellTask(app: Context, address: String, task: String, fields: Map<String, Any?>) =
            PhoneEventBroadcast.send(app, PhoneEvents.Kind.TASK_FINISHED, address, fields + ("task" to task))

        private fun tellPress(app: Context, address: String, label: String, fields: Map<String, Any?>) =
            PhoneEventBroadcast.send(app, PhoneEvents.Kind.DECK_PRESSED, address, fields + ("label" to label))
    }
}
