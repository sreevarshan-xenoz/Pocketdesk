package dev.pocketdesk.android.ui

/**
 * When a dropped session reconnects by itself (R2), kept apart from Android
 * so it is tested. Only a link that went away is retried: the desktop's
 * server restarting, the computer waking, the network coming back. An end the
 * desktop chose (a kick, a refusal, an unpairing) and anything about trust or
 * pairing wait for the person, on the card.
 */
object AutoReconnect {
    /** How long a session keeps trying before it leaves it to the person. */
    const val GIVE_UP_MS = 10 * 60_000L

    private const val FIRST_MS = 2_000L
    private const val MAX_MS = 30_000L

    fun retryable(reason: String): Boolean {
        val r = reason.lowercase()
        if ("closed by the desktop" in r) return false
        if (errorSuggestsRepair(reason) || "no cached wan identity" in r) return false
        return listOf("stalled", "timed out", "timeout", "refused", "unreachable", "no route", "reset", "closed")
            .any { it in r }
    }

    /**
     * How long to wait before attempt [attempt] (0-based), [elapsedMs] after
     * the link dropped; null once it is time to stop.
     */
    fun delayMs(attempt: Int, elapsedMs: Long): Long? {
        if (elapsedMs >= GIVE_UP_MS) return null
        val doubled = FIRST_MS shl attempt.coerceAtMost(10)
        return doubled.coerceAtMost(MAX_MS)
    }
}
