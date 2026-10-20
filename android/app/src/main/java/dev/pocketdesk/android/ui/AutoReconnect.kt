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

    /** How long a picture must have played before a later drop starts the backoff over. */
    const val HEALTHY_MS = 10_000L

    private const val FIRST_MS = 2_000L
    private const val MAX_MS = 30_000L

    private const val DESKTOP_CLOSED = "closed by the desktop:"

    /** The reasons the desktop gives when it ends a session on purpose. */
    private val DESKTOP_ENDS =
        listOf("kicked", "refused", "not paired", "permissions changed", "session end", "not allowed")

    // "handshake eof": the far side closed while TLS was still opening, as a
    // server stopping or starting does; never a refused certificate (I1).
    private val TRANSIENT =
        listOf("stalled", "timed out", "timeout", "refused", "unreachable", "no route", "reset", "closed", "handshake eof")

    fun retryable(reason: String): Boolean {
        val r = reason.lowercase()
        // The bridge reports transport errors (a timeout, a reset) the same
        // way as the desktop's own close; only the desktop's words are final.
        if (DESKTOP_CLOSED in r) return DESKTOP_ENDS.none { it in r.substringAfter(DESKTOP_CLOSED) }
        if (errorSuggestsRepair(reason)) return false
        return TRANSIENT.any { it in r }
    }

    /**
     * Why a sound session or a remote ended, in the words [retryable] reads:
     * the computer's close reason [closed] when it gave one, a silent link
     * otherwise; null when it ended while the owner was still to answer
     * ([wasWaiting]), which is theirs to decide again (I1).
     */
    fun endedBy(closed: String?, wasWaiting: Boolean): String? =
        when {
            closed != null -> "stream $DESKTOP_CLOSED $closed"
            wasWaiting -> null
            else -> "stalled"
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
