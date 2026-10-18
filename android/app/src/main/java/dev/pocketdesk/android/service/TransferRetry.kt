package dev.pocketdesk.android.service

import dev.pocketdesk.android.doctor.AddressKind
import dev.pocketdesk.android.doctor.ConnectionChecks

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
     * How long the transfers wait, in all, for a network during one outage
     * (R6i): a phone's Wi-Fi can take a minute to rejoin, longer than the
     * retries. [OfflineBudget] keeps the count across jobs.
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
        /** Wi-Fi or Ethernet: a computer at home may be on it. */
        val local: Boolean,
        /** Mobile data. */
        val cellular: Boolean,
        /** Up but carrying nothing (a mobile network during a call). */
        val suspended: Boolean,
        /**
         * It reaches the internet: a mobile network for calls (IMS) or
         * picture messages (MMS) does not.
         */
        val internet: Boolean,
    )

    /**
     * Whether the phone has a network a transfer with its computer may use
     * (R6i): Wi-Fi or Ethernet that is not suspended, or its own hotspot or
     * tethering ([sharing]), which a computer may be on; and mobile data
     * with the internet, but only for a computer it can reach from away
     * ([fromAway]): by the internet road (2c), or at an address that is
     * not a home one.
     */
    fun hasNetwork(networks: List<SeenNetwork>, sharing: Boolean, fromAway: Boolean): Boolean =
        sharing || networks.any { n ->
            !n.vpn && !n.suspended && (n.local || (n.cellular && n.internet && fromAway))
        }

    /**
     * Whether the computer at [address] can be reached from away: the phone
     * holds its internet identity ([wanIdentity]), or the address is not a
     * home one (private, link-local or `.local`).
     */
    fun reachedFromAway(address: String, wanIdentity: Boolean): Boolean {
        if (wanIdentity) return true
        return when (ConnectionChecks.kindOf(address)) {
            AddressKind.PRIVATE_V4, AddressKind.LINK_LOCAL -> false
            AddressKind.HOSTNAME -> !address.lowercase().trimEnd('.').endsWith(".local")
            else -> true
        }
    }

    /**
     * The wait for a network, shared by every transfer during one outage
     * (R6i): a folder from the computer comes as one download per file, and
     * each would otherwise wait its own 3 minutes. A transfer that succeeds,
     * or one that starts with a network, ends the outage; so do ten minutes
     * without a wait, so a transfer asked for much later waits again.
     * Clock in milliseconds, given.
     */
    class OfflineBudget(private val forgetAfterMs: Long = 10 * 60_000L) {
        private var waitedSeconds = 0L
        private var lastWaitMs: Long? = null

        /** Seconds waited in this outage, as of [nowMs]. */
        fun waited(nowMs: Long): Long {
            val last = lastWaitMs
            if (last != null && nowMs - last >= forgetAfterMs) reset()
            return waitedSeconds
        }

        /** A wait of [seconds] ended at [nowMs]. */
        fun add(seconds: Long, nowMs: Long) {
            waitedSeconds += seconds
            lastWaitMs = nowMs
        }

        /** The outage is over. */
        fun reset() {
            waitedSeconds = 0
            lastWaitMs = null
        }
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
        // Cancelled on the phone, or on the computer (C1).
        CANCELLED_ON_PHONE,
        CANCELLED_ON_COMPUTER,
    )

    /** What a transfer the phone's Cancel stopped fails with (the bridge's words too). */
    const val CANCELLED_ON_PHONE = "cancelled on the phone"

    /** The computer's refusal of a file its owner cancelled (C1). */
    const val CANCELLED_ON_COMPUTER = "cancelled on the computer"

    /**
     * Refusals that pass: while other transfers run, or a request that went
     * by the internet road while the home network came back (the computer
     * dials a download back on the home network only).
     */
    private val BUSY = listOf("busy", "still running", "need the local network")
}
