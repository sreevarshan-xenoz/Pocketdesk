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

    /** The phone itself will not take this file (no room, an unsafe place): never retried. */
    class Refusal(message: String) : Exception(message)

    /** [delayBefore] for a failure [error]: a [Refusal] is final, whatever it says. */
    fun delayAfter(attempt: Int, error: Throwable?, enabled: Boolean): Long? =
        if (error is Refusal) null else delayBefore(attempt, error?.message, enabled)

    /**
     * How long one job (a share, a folder, a download) waits, in all, for a
     * network (R6i): a phone's Wi-Fi can take a minute to rejoin, longer
     * than the retries. Once spent, its other files do not wait again.
     */
    const val NETWORK_WAIT_SECONDS = 180L

    /** What a transfer does after a failed attempt. */
    sealed interface Next {
        /** Try again in [seconds], as the next retry. */
        data class After(val seconds: Long) : Next

        /** Wait up to [seconds] for the network, then try again; the wait uses no retry. */
        data class WhenOnline(val seconds: Long) : Next

        /** Stop; [noNetwork] when it was the network that never came back. */
        data class Stop(val noNetwork: Boolean) : Next
    }

    /**
     * After attempt [attempt] failed with [error]: when the phone has no
     * network at all ([hasNetwork]), it waits for one, up to
     * [NETWORK_WAIT_SECONDS] in all for the job ([waitedOfflineSeconds] so
     * far), without spending a retry; otherwise it retries as [delayAfter]
     * says.
     */
    fun next(
        attempt: Int,
        error: Throwable?,
        enabled: Boolean,
        hasNetwork: Boolean,
        waitedOfflineSeconds: Long,
    ): Next {
        val final = error is Refusal || error?.message?.let(::isFinal) == true
        if (!enabled || final) return Next.Stop(noNetwork = false)
        if (!hasNetwork) {
            val left = NETWORK_WAIT_SECONDS - waitedOfflineSeconds
            return if (left > 0) Next.WhenOnline(left) else Next.Stop(noNetwork = true)
        }
        return delayAfter(attempt, error, enabled)?.let { Next.After(it) } ?: Next.Stop(noNetwork = false)
    }

    /** One of the phone's networks, as [hasNetwork] reads it. */
    data class SeenNetwork(
        /** A VPN, which rides another network and is none by itself. */
        val vpn: Boolean,
        /** Wi-Fi, Ethernet or mobile data. */
        val carrier: Boolean,
        /** Up but carrying nothing (a mobile network during a call). */
        val suspended: Boolean,
    )

    /**
     * Whether the phone has a network a transfer may use (R6i): Wi-Fi,
     * Ethernet or mobile data that is not suspended, or its own hotspot or
     * tethering ([sharing]), which a computer may be on. Mobile data counts
     * even for a computer at home, since files reach it by the internet
     * road too (2c); whether that road is there is the retries' to find.
     */
    fun hasNetwork(networks: List<SeenNetwork>, sharing: Boolean): Boolean =
        sharing || networks.any { !it.vpn && it.carrier && !it.suspended }

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

    /**
     * Refusals that pass: while other transfers run, or a request that went
     * by the internet road while the home network came back (the computer
     * dials a download back on the home network only).
     */
    private val BUSY = listOf("busy", "still running", "need the local network")
}
