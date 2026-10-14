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
        if (error != null && FINAL.any { error.contains(it, ignoreCase = true) }) return null
        return DELAYS_SECONDS[attempt - 1]
    }

    /** Failures a retry would only repeat. */
    private val FINAL = listOf(
        "refused",
        "is empty",
        "cannot open",
        "larger than",
        "permission",
    )
}
