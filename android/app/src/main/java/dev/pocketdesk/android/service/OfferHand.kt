package dev.pocketdesk.android.service

/**
 * The computer's offers this process has in hand, by id (S2): the computer
 * offers a phone's open offers again when its link comes back, and one in
 * hand is neither fetched twice nor offered for a tap twice.
 *
 * A claim (taken from the bridge, about to start or waiting for a tap) lasts
 * [CLAIM_MS], an offer's time on the computer; a running fetch lasts until
 * [release]. A process that dies forgets them all, which is what lets an
 * offer it lost be taken again.
 */
class OfferHand {
    private val held = HashMap<String, Long>()

    /** Take offer [id] at [nowMs]; false while it is already in hand. */
    @Synchronized
    fun claim(id: String, nowMs: Long): Boolean {
        lapse(nowMs)
        if (id in held) return false
        held[id] = nowMs + CLAIM_MS
        return true
    }

    /** Its fetch starts; false while one runs already. */
    @Synchronized
    fun run(id: String, nowMs: Long): Boolean {
        lapse(nowMs)
        if (held[id] == RUNNING) return false
        held[id] = RUNNING
        return true
    }

    /** Its fetch ended, however it went. */
    @Synchronized
    fun release(id: String) {
        held.remove(id)
    }

    private fun lapse(nowMs: Long) {
        held.entries.removeAll { it.value != RUNNING && it.value <= nowMs }
    }

    companion object {
        /** An offer's time on the computer (`offers::OFFER_TTL`). */
        const val CLAIM_MS = 10 * 60_000L
        private const val RUNNING = Long.MAX_VALUE

        /** This process's. */
        val shared = OfferHand()
    }
}
