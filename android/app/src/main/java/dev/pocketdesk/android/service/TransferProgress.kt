package dev.pocketdesk.android.service

/** The numbers a transfer notification shows (F7), kept apart from Android so they are tested. */
object TransferProgress {
    /** Whole percent of [sent] over [total]; 0 for an unknown total, never over 100. */
    fun percent(sent: Long, total: Long): Int {
        if (total <= 0 || sent <= 0) return 0
        return ((sent.coerceAtMost(total) * 100) / total).toInt()
    }

    /**
     * The bridge's counters belong to whichever transfer ran last, so they
     * count only once they describe this file: the same total, and no more
     * sent than that.
     */
    fun forFile(sent: Long, total: Long, fileSize: Long): Int =
        if (fileSize > 0 && total == fileSize) percent(sent, total) else 0

    /**
     * Bytes a second over the last [windowMs] of the readings it is given
     * (C1): what moved between the oldest kept and the newest. A reading
     * lower than the last (a new attempt resuming) starts it over.
     */
    class Speed(private val windowMs: Long = 4_000L) {
        private val readings = ArrayDeque<Pair<Long, Long>>()

        /** Add the reading [done] bytes at [atMs]; the speed now, 0 until it can tell. */
        fun add(atMs: Long, done: Long): Long {
            // A reading lower than the last, or the first above nothing (a
            // resumed attempt counts what it already had at once): start over.
            if (readings.lastOrNull()?.let { it.second > done || (it.second == 0L && done > 0L) } == true) readings.clear()
            readings.addLast(atMs to done)
            while (readings.size > 1 && atMs - readings.first().first > windowMs) readings.removeFirst()
            val (firstAt, firstDone) = readings.first()
            val span = atMs - firstAt
            return if (span < 500) 0 else (done - firstDone) * 1000 / span
        }
    }
}
