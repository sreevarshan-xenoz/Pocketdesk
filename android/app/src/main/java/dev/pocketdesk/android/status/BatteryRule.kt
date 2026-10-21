package dev.pocketdesk.android.status

/**
 * When the phone tells the computer its battery (T8b), and what it says:
 * KDE Connect's `kdeconnect.battery`, with the phone's state beside it
 * (L9, [PhoneStateRule]). Kept apart from Android so it is tested.
 */
object BatteryRule {
    /** A report is sent again after this long even if nothing changed, so its age stays fresh. */
    const val RESEND_MS = 5 * 60_000L

    data class Report(
        val charge: Int,
        val charging: Boolean,
        val low: Boolean,
        val state: PhoneStateRule.State = PhoneStateRule.State(),
    )

    /** The report for Android's battery reading, or null for a reading that makes no sense. */
    fun report(level: Int, scale: Int, plugged: Int, low: Boolean): Report? {
        if (scale <= 0 || level < 0 || level > scale) return null
        val charge = Math.round(level * 100f / scale)
        return Report(charge = charge, charging = plugged != 0, low = low)
    }

    /** BatteryManager's status words (BATTERY_STATUS_*), kept here so the rule is tested. */
    const val STATUS_CHARGING = 2
    const val STATUS_DISCHARGING = 3
    const val STATUS_NOT_CHARGING = 4
    const val STATUS_FULL = 5

    /**
     * The report with the battery as BatteryManager says it now: its
     * [capacity] (0–100, else unknown) and [status] over the sticky
     * broadcast's, which ColorOS was seen to leave stale for an app in the
     * background (the computer kept 72 % while the phone was at 79 %). Either
     * unknown keeps the broadcast's; with no broadcast, the capacity alone.
     */
    fun fresh(sticky: Report?, capacity: Int, status: Int): Report? {
        val charge = capacity.takeIf { it in 0..100 }
        val charging = when (status) {
            STATUS_CHARGING, STATUS_FULL -> true
            STATUS_DISCHARGING, STATUS_NOT_CHARGING -> false
            else -> null
        }
        if (sticky == null) {
            return charge?.let { Report(charge = it, charging = charging ?: false, low = false) }
        }
        return sticky.copy(charge = charge ?: sticky.charge, charging = charging ?: sticky.charging)
    }

    /**
     * Whether to send [now] at [nowMs], [last] having been sent at [lastMs]:
     * a change in the battery, or in the state [PhoneStateRule.changedNow]
     * names, at once; storage and bars with the resend.
     */
    fun shouldSend(last: Report?, lastMs: Long, now: Report, nowMs: Long): Boolean =
        last == null ||
            last.copy(state = PhoneStateRule.State()) != now.copy(state = PhoneStateRule.State()) ||
            PhoneStateRule.changedNow(last.state, now.state) ||
            nowMs - lastMs >= RESEND_MS

    /** The packet's body. */
    fun body(report: Report): String =
        "{\"currentCharge\":${report.charge},\"isCharging\":${report.charging}," +
            "\"thresholdEvent\":${if (report.low) 1 else 0}${PhoneStateRule.fields(report.state)}}"
}
