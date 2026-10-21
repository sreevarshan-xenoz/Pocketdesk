package dev.pocketdesk.android.media

/**
 * What the background reporter knows between reports (N2), kept apart
 * from Android so it is tested: what plays, since when it is paused, what
 * the link heard, and which cover the computer holds.
 */
class BackgroundReport {
    /** What this link last heard. */
    var sent = MediaRule.Sent()
        private set

    /** The cover the computer holds for this phone: the last one sent. */
    var heldArt: String? = null
        private set

    /** What plays now, as the players last said. */
    var seen: MediaRule.Media? = null
        private set

    /** Since when [seen] is paused, by the player's own clock; null while it plays. */
    var pausedSinceMs: Long? = null
        private set

    private var linkedTo: Pair<String, Int>? = null

    /**
     * The control link is to [peer] (null: none), [linked] meaning to the
     * own computer: a new or lost link knows nothing that was sent. True
     * when the link changed.
     */
    fun link(peer: Pair<String, Int>?, linked: Boolean): Boolean {
        val changed = peer != linkedTo
        if (changed || !linked) {
            linkedTo = peer
            sent = MediaRule.Sent()
            heldArt = null
        }
        return changed
    }

    /** What plays [now], paused since [pausedSinceMs] when it is paused. */
    fun see(now: MediaRule.Media?, pausedSinceMs: Long?) {
        seen = now
        this.pausedSinceMs = if (now == null || now.isPlaying) null else pausedSinceMs
    }

    /** Whether what plays is current at [nowMs], so a link is kept for it. */
    fun current(nowMs: Long): Boolean = MediaRule.current(seen, nowMs, pausedSinceMs)

    /** Whether a report is due at [nowMs]. */
    fun due(nowMs: Long): Boolean = MediaRule.backgroundDue(sent, seen, nowMs, pausedSinceMs)

    /** Whether the cover's bytes go with the report due. */
    fun withArt(): Boolean = MediaRule.sendArt(seen?.artId, heldArt)

    /** The report was sent at [nowMs]: the computer holds its cover, or none. */
    fun sent(nowMs: Long) {
        sent = MediaRule.Sent(any = true, last = seen, atMs = nowMs)
        heldArt = seen?.artId
    }

    /** Forget all: the reporter stopped. */
    fun forget() {
        sent = MediaRule.Sent()
        heldArt = null
        seen = null
        pausedSinceMs = null
        linkedTo = null
    }
}
