package dev.pocketdesk.android.status

/**
 * When the phone tells the computer its battery (T8b), and what it says:
 * KDE Connect's `kdeconnect.battery`. Kept apart from Android so it is
 * tested.
 */
object BatteryRule {
    /** A report is sent again after this long even if nothing changed, so its age stays fresh. */
    const val RESEND_MS = 5 * 60_000L

    data class Report(val charge: Int, val charging: Boolean, val low: Boolean)

    /** The report for Android's battery reading, or null for a reading that makes no sense. */
    fun report(level: Int, scale: Int, plugged: Int, low: Boolean): Report? {
        if (scale <= 0 || level < 0 || level > scale) return null
        val charge = Math.round(level * 100f / scale)
        return Report(charge = charge, charging = plugged != 0, low = low)
    }

    /** Whether to send [now] at [nowMs], [last] having been sent at [lastMs]. */
    fun shouldSend(last: Report?, lastMs: Long, now: Report, nowMs: Long): Boolean =
        last != now || nowMs - lastMs >= RESEND_MS

    /** The packet's body. */
    fun body(report: Report): String =
        "{\"currentCharge\":${report.charge},\"isCharging\":${report.charging}," +
            "\"thresholdEvent\":${if (report.low) 1 else 0}}"
}
