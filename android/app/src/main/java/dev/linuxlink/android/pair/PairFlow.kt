package dev.linuxlink.android.pair

import dev.linuxlink.android.bridge.RustCore

/** What pairing from a [PairLink] needs from the bridge; faked in tests. */
interface PairOps {
    fun pinCertificate(address: String, fingerprint: String): Result<Unit>
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
 * trusts a different computer there) is skipped, never contacted. The PIN
 * itself still travels over the control channel, which is not encrypted —
 * the same exposure as typing the PIN by hand.
 *
 * A desktop that answers and refuses the PIN ends the attempt: every
 * address is the same desktop, and the fix is a fresh code on its screen.
 */
fun runPairing(link: PairLink, ops: PairOps): PairOutcome {
    var lastError = "no address to try"
    for (address in link.addresses) {
        val fp = link.fingerprint
        if (fp != null) {
            val pinned = ops.pinCertificate(address, fp)
            if (pinned.isFailure) {
                lastError = pinned.exceptionOrNull()?.message ?: "could not pin"
                continue
            }
        }
        val connected = ops.connect(address, link.controlPort)
        if (connected.isFailure) {
            lastError = connected.exceptionOrNull()?.message ?: "unreachable"
            continue
        }
        val paired = ops.pair(address, link.controlPort, link.pin)
        paired.onFailure {
            lastError = it.message ?: "pairing failed"
        }.onSuccess { serverId ->
            return if (serverId != null) PairOutcome.Paired(address, serverId) else PairOutcome.Expired
        }
    }
    return PairOutcome.Unreachable(lastError)
}

/** [PairOps] over the Rust bridge. Blocking: call off the main thread. */
object RustPairOps : PairOps {
    override fun pinCertificate(address: String, fingerprint: String) =
        RustCore.pinPeerCertificate(address, fingerprint)

    override fun connect(address: String, controlPort: Int): Result<Unit> =
        RustCore.connectToPeer(address, controlPort).map { }

    override fun pair(address: String, controlPort: Int, pin: String): Result<String?> =
        RustCore.pairWithPin(address, controlPort, pin)
}
