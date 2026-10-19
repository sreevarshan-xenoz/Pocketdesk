package dev.pocketdesk.android.ui

import org.json.JSONObject

/**
 * The computer's state as the phone shows it (F2): CPU, memory, disks,
 * battery and uptime. The rules are kept apart from Android so they are
 * tested; [parse] only reads the JSON into them.
 */
object ComputerState {
    /** Used of total bytes, as a bar's share and its words. */
    data class Space(val name: String, val used: Long, val total: Long) {
        /** The bar's share, 0 to 1. */
        val fraction: Float get() = if (total <= 0) 0f else (used.toDouble() / total).toFloat().coerceIn(0f, 1f)
        /** "8.0 GB of 16.0 GB". */
        fun words(of: String): String = "${RemoteFiles.humanSize(used)} $of ${RemoteFiles.humanSize(total)}"
    }

    /** What the computer said; a part it could not read is null. */
    data class State(
        val off: Boolean = false,
        val cpu: Float? = null,
        val memory: Space? = null,
        val disks: List<Space> = emptyList(),
        val batteryCharge: Int? = null,
        val batteryCharging: Boolean = false,
        val uptimeSecs: Long? = null,
    )

    /** Uptime in the largest two units: "3 d 4 h", "5 h 12 min", "7 min". */
    fun uptime(secs: Long, day: String, hour: String, minute: String): String {
        val days = secs / 86_400
        val hours = secs % 86_400 / 3_600
        val minutes = secs % 3_600 / 60
        return when {
            days > 0 -> "$days $day $hours $hour"
            hours > 0 -> "$hours $hour $minutes $minute"
            else -> "$minutes $minute"
        }
    }

    /** A CPU reading as a percentage, or null when it is not a number. */
    fun cpu(value: Double): Float? = value.takeIf { !it.isNaN() }?.toFloat()?.coerceIn(0f, 100f)

    /** A space from its parts, or null when it says nothing. */
    fun space(name: String, used: Long, total: Long): Space? =
        if (total <= 0 || used < 0) null else Space(name, minOf(used, total), total)

    /** The computer's answer. */
    fun parse(json: String): State {
        val body = JSONObject(json)
        if (body.optBoolean("off", false)) return State(off = true)
        val memory = body.optJSONObject("memory")?.let { space("", it.optLong("used", -1), it.optLong("total", -1)) }
        val disks = body.optJSONArray("disks")?.let { array ->
            (0 until array.length()).mapNotNull { i ->
                array.optJSONObject(i)?.let { space(it.optString("name"), it.optLong("used", -1), it.optLong("total", -1)) }
            }
        }.orEmpty()
        val battery = body.optJSONObject("battery")
        return State(
            // optDouble is NaN for a value that is not a number (F2 review).
            cpu = if (body.isNull("cpu") || !body.has("cpu")) null else cpu(body.optDouble("cpu")),
            memory = memory,
            disks = disks,
            batteryCharge = battery?.optInt("charge", -1)?.takeIf { it in 0..100 },
            batteryCharging = battery?.optBoolean("charging", false) == true,
            uptimeSecs = if (body.isNull("uptime_secs") || !body.has("uptime_secs")) null else body.optLong("uptime_secs"),
        )
    }
}
