package dev.pocketdesk.android.pair

import android.content.Context
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore

/** What pairing from a [PairLink] needs from the bridge; faked in tests. */
interface PairOps {
    /** Pin [fingerprint] for [address]; true when the pin is new (not already there). */
    fun pinCertificate(address: String, fingerprint: String): Result<Boolean>

    /** Take back a pin this attempt added. */
    fun forgetCertificate(address: String)

    /** Whether a computer is saved at [address] (its pin is its card's). */
    fun hasCard(address: String): Boolean

    fun connect(address: String, controlPort: Int): Result<Unit>

    /** The desktop's device id once paired; null when the PIN was refused. */
    fun pair(address: String, controlPort: Int, pin: String): Result<String?>
}

sealed interface PairOutcome {
    data class Paired(val address: String, val serverId: String) : PairOutcome

    /** The desktop answered but refused the PIN: the code on screen expired. */
    data object Expired : PairOutcome

    /** No address in the code answered; [lastError] is the last failure. */
    data class Unreachable(val lastError: String) : PairOutcome
}

/**
 * Pair with the desktop a code describes. Addresses are tried best first.
 * Each is first pinned to the code's certificate fingerprint, so the phone's
 * streaming (QUIC) connections to it are verified from the start instead of
 * trusted on first use; an address whose pin is refused (this phone already
 * trusts a different computer there) is skipped, never contacted, and a pin
 * this attempt added is taken back when pairing at that address does not
 * succeed — a code that did not pair leaves no trust behind. The PIN
 * itself still travels over the control channel, which is not encrypted —
 * the same exposure as typing the PIN by hand.
 *
 * A desktop that answers and refuses the PIN ends the attempt: every
 * address is the same desktop, and the fix is a fresh code on its screen.
 */
fun runPairing(link: PairLink, ops: PairOps): PairOutcome {
    var lastError = "no address to try"
    for (address in link.addresses) {
        var added = false
        val fp = link.fingerprint
        if (fp != null) {
            val pinned = ops.pinCertificate(address, fp)
            if (pinned.isFailure) {
                lastError = pinned.exceptionOrNull()?.message ?: "could not pin"
                continue
            }
            added = pinned.getOrDefault(false)
        }
        // With no fingerprint the connection pins whatever answers: taken
        // back too, unless it is a saved computer's (BC3: it stayed with no
        // card, and another computer there could not be paired).
        val undo = { if (added || (fp == null && !ops.hasCard(address))) ops.forgetCertificate(address) }
        val connected = ops.connect(address, link.controlPort)
        if (connected.isFailure) {
            lastError = connected.exceptionOrNull()?.message ?: "unreachable"
            undo()
            continue
        }
        val paired = ops.pair(address, link.controlPort, link.pin)
        val serverId = paired.getOrNull()
        if (serverId != null) return PairOutcome.Paired(address, serverId)
        undo()
        if (paired.isSuccess) return PairOutcome.Expired
        lastError = paired.exceptionOrNull()?.message ?: "pairing failed"
    }
    return PairOutcome.Unreachable(lastError)
}

/** [PairOps] over the Rust bridge. Blocking: call off the main thread. */
class RustPairOps(private val context: Context) : PairOps {
    override fun pinCertificate(address: String, fingerprint: String) =
        RustCore.pinPeerCertificate(address, fingerprint)

    override fun forgetCertificate(address: String) {
        RustCore.forgetPeerCertificate(address)
    }

    override fun hasCard(address: String) = HostStore.hosts(context).any { it.address == address }

    override fun connect(address: String, controlPort: Int): Result<Unit> =
        RustCore.connectToPeer(address, controlPort).map { }

    override fun pair(address: String, controlPort: Int, pin: String): Result<String?> =
        RustCore.pairWithPin(address, controlPort, pin)
}
