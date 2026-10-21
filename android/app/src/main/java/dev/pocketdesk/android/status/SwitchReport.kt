package dev.pocketdesk.android.status

/**
 * What the phone tells its own computer of its features (L7): for each on
 * a fixed list, its own switch and whether Android lets it work (null where
 * Android needs nothing). Booleans only: no app, folder or package names.
 * Kept apart from Android so it is tested; [readSwitches] reads them.
 */
object SwitchReport {
    const val SWITCHES = "kdeconnect.pocketdesk.switches"

    /** The features, in the order the computer shows them (its own list matches). */
    val FEATURES = listOf(
        "notifications", "answers", "texts", "clipboard", "files", "folders", "photos",
        "screenshots", "camera", "links", "ring", "media", "media_background", "computer_media", "mute", "control", "dial",
        "commands",
    )

    data class Switch(val on: Boolean, val android: Boolean?)

    /** Whether to send [now] at [nowMs], [last] having been sent at [lastMs]: on a change, else with the battery's resend. */
    fun shouldSend(last: Map<String, Switch>?, lastMs: Long, now: Map<String, Switch>, nowMs: Long): Boolean =
        last != now || nowMs - lastMs >= BatteryRule.RESEND_MS

    /** The packet's body: only the known features, in their order. */
    fun body(switches: Map<String, Switch>): String = buildString {
        append("{\"switches\":{")
        FEATURES.filter { it in switches }.forEachIndexed { i, name ->
            val s = switches.getValue(name)
            if (i > 0) append(',')
            append("\"$name\":{\"on\":${s.on}")
            s.android?.let { append(",\"android\":$it") }
            append('}')
        }
        append("}}")
    }
}

/**
 * Sends [SwitchReport] to the phone's own computer from the battery's ticks
 * (L7), under the same switch: on a change, else every five minutes.
 */
class SwitchSender {
    private var last: Map<String, SwitchReport.Switch>? = null
    private var lastMs = 0L

    /** A new link is told again at once: the computer may have restarted. */
    fun forget() {
        last = null
    }

    fun tick(now: Map<String, SwitchReport.Switch>, nowMs: Long, send: (String) -> Result<*>) {
        if (!SwitchReport.shouldSend(last, lastMs, now, nowMs)) return
        if (send(SwitchReport.body(now)).isSuccess) {
            last = now
            lastMs = nowMs
        }
    }
}
