package dev.pocketdesk.android.ring

/**
 * What the phone does with a ring from its computer (N1), as a rule the
 * notification listener's tick applies: the bridge's latch is always taken
 * outside a session (a ring left in it would sound when a session next
 * opens), and rings only with "Ring from the computer" on, at most once a
 * minute here too.
 */
class RingRule(private val everyMs: Long = EVERY_MS) {
    private var lastRingMs: Long? = null

    enum class Act { NOTHING, TAKE_AND_DROP, RING }

    /**
     * What to do at [nowMs] when [asked] (the latch was set), with the
     * switch [on] and a session [inSession] (whose own watcher rings).
     */
    fun decide(asked: Boolean, on: Boolean, inSession: Boolean, nowMs: Long): Act {
        if (inSession || !asked) return Act.NOTHING
        if (!on) return Act.TAKE_AND_DROP
        val last = lastRingMs
        if (last != null && nowMs - last < everyMs) return Act.TAKE_AND_DROP
        lastRingMs = nowMs
        return Act.RING
    }

    companion object {
        const val EVERY_MS = 60_000L
    }
}
