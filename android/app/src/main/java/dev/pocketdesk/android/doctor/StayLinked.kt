package dev.pocketdesk.android.doctor

/**
 * Whether the phone keeps its link foreground service (M1): with the
 * switch on, a feature that runs on the notification listener on, access
 * granted to run it, and a computer to be linked to.
 */
object StayLinked {
    fun wanted(on: Boolean, features: Boolean, access: Boolean, paired: Boolean): Boolean =
        on && features && access && paired
}

/**
 * Notices the process was stopped while the phone ran (M1): a phone maker's
 * freezer (ColorOS's freezes the whole app with the screen off), seen as a
 * gap between the listener's 2 s ticks. `uptimeMillis` stops while the CPU
 * sleeps, so a phone merely asleep makes no gap; only a CPU that ran while
 * the app did not does.
 */
object FreezeWatch {
    /** Uptime between two ticks that counts as a freeze. */
    const val GAP_MS = 30_000L

    /** How long a freeze seen is told on the Health card. */
    const val RECENT_MS = 24 * 60 * 60 * 1000L

    fun frozen(previousUptime: Long?, uptime: Long): Boolean =
        previousUptime != null && uptime - previousUptime >= GAP_MS

    /** Whether a freeze seen at [lastFreezeMs] (wall clock) is still news. */
    fun recent(lastFreezeMs: Long?, nowMs: Long): Boolean =
        lastFreezeMs != null && lastFreezeMs <= nowMs && nowMs - lastFreezeMs <= RECENT_MS
}
