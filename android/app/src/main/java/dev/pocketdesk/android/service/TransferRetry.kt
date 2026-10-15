package dev.pocketdesk.android.service

/**
 * When a failed file is sent again (T6a). A retry continues where the last
 * attempt stopped, since the computer keeps what arrived, so retrying is
 * cheap; it is not worth it when the computer said no or the file cannot
 * be sent at all.
 */
object TransferRetry {
    /** Seconds to wait before each retry, in order. */
    val DELAYS_SECONDS = listOf(5L, 15L, 45L)

    /**
     * The seconds to wait before retry number [attempt] (1 for the first
     * retry) after [error], or null for no more.
     */
    fun delayBefore(attempt: Int, error: String?, enabled: Boolean): Long? {
        if (!enabled || attempt < 1 || attempt > DELAYS_SECONDS.size) return null
        if (error != null && isFinal(error)) return null
        return DELAYS_SECONDS[attempt - 1]
    }

    /**
     * Whether a folder send goes on (T6b). It stops once [GIVE_UP_AFTER]
     * files in a row failed, each after its own retries, for a reason that
     * is not the file's own (the computer stopped answering): going on
     * would only try every other file the same way, for hours. A file the
     * computer refused or that cannot be read does not count.
     */
    class FolderStreak {
        private var failedInARow = 0

        /** Record one file's outcome; false when the send should stop. */
        fun record(sent: Boolean, error: String?): Boolean {
            failedInARow = when {
                sent -> 0
                error != null && isFinal(error) -> failedInARow
                else -> failedInARow + 1
            }
            return failedInARow < GIVE_UP_AFTER
        }

        companion object {
            const val GIVE_UP_AFTER = 3
        }
    }

    private fun isFinal(error: String): Boolean {
        // A computer that refused only because it was busy may take it later.
        if (BUSY.any { error.contains(it, ignoreCase = true) }) return false
        return FINAL.any { error.contains(it, ignoreCase = true) }
    }

    /** Failures a retry would only repeat. */
    private val FINAL = listOf(
        // The computer's own refusals ("The computer refused the file: …"),
        // not the network's "connection refused", which a retry may outlast.
        "computer refused",
        "is empty",
        "empty file",
        "cannot open",
        "larger than",
        "not accepted",
        "permission",
    )

    /** Refusals that last only while other transfers run. */
    private val BUSY = listOf("busy", "still running")
}
