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
}
