package dev.pocketdesk.android.pair

import org.junit.Assert.assertEquals
import org.junit.Test

class PairFlowTest {
    private val link = PairLink(
        name = "Desk", deviceId = "d-1",
        addresses = listOf("172.20.0.1", "192.168.1.5"),
        controlPort = 1716, streamPort = 4716, pin = "123456",
        fingerprint = "ab".repeat(32), endpoint = null,
    )

    /**
     * Records calls; [reachable] answers, [accept] decides the PIN, [pinRefused]
     * fail pinning, [alreadyPinned] were pinned (same fingerprint) before.
     */
    private class Fake(
        val reachable: Set<String>,
        val accept: Boolean = true,
        val pinRefused: Set<String> = emptySet(),
        val alreadyPinned: Set<String> = emptySet(),
        val saved: Set<String> = emptySet(),
    ) : PairOps {
        val calls = mutableListOf<String>()
        override fun pinCertificate(address: String, fingerprint: String): Result<Boolean> {
            calls += "pin $address"
            return if (address in pinRefused) {
                Result.failure(Exception("trusts a different computer"))
            } else {
                Result.success(address !in alreadyPinned)
            }
        }
        override fun forgetCertificate(address: String) {
            calls += "forget $address"
        }
        override fun hasCard(address: String) = address in saved
        override fun connect(address: String, controlPort: Int): Result<Unit> {
            calls += "connect $address"
            return if (address in reachable) Result.success(Unit) else Result.failure(Exception("timeout"))
        }
        override fun pair(address: String, controlPort: Int, pin: String): Result<String?> {
            calls += "pair $address"
            return Result.success(if (accept) "server-1" else null)
        }
    }

    @Test
    fun theFirstReachableAddressIsPinnedThenPaired() {
        val ops = Fake(reachable = setOf("192.168.1.5"))
        assertEquals(PairOutcome.Paired("192.168.1.5", "server-1"), runPairing(link, ops))
        assertEquals(
            listOf(
                "pin 172.20.0.1", "connect 172.20.0.1", "forget 172.20.0.1",
                "pin 192.168.1.5", "connect 192.168.1.5", "pair 192.168.1.5",
            ),
            ops.calls,
        )
    }

    @Test
    fun aPinMadeForAFailedAttemptIsRemovedButAnOlderOneIsKept() {
        val refused = Fake(reachable = setOf("172.20.0.1"), accept = false)
        assertEquals(PairOutcome.Expired, runPairing(link, refused))
        assertEquals(listOf("pin 172.20.0.1", "connect 172.20.0.1", "pair 172.20.0.1", "forget 172.20.0.1"), refused.calls)

        val known = Fake(reachable = emptySet(), alreadyPinned = setOf("172.20.0.1", "192.168.1.5"))
        runPairing(link, known)
        assertEquals(false, known.calls.any { it.startsWith("forget") })
    }

    @Test
    fun aRejectedPinIsAnExpiredCodeAndStopsTrying() {
        val ops = Fake(reachable = setOf("172.20.0.1", "192.168.1.5"), accept = false)
        assertEquals(PairOutcome.Expired, runPairing(link, ops))
        assertEquals(
            listOf("pin 172.20.0.1", "connect 172.20.0.1", "pair 172.20.0.1", "forget 172.20.0.1"),
            ops.calls,
        )
    }

    @Test
    fun noReachableAddressIsUnreachable() {
        val outcome = runPairing(link, Fake(reachable = emptySet()))
        assertEquals(PairOutcome.Unreachable("timeout"), outcome)
    }

    @Test
    fun anAddressWhosePinIsRefusedIsNeverConnectedTo() {
        val ops = Fake(reachable = setOf("172.20.0.1", "192.168.1.5"), pinRefused = setOf("172.20.0.1"))
        assertEquals(PairOutcome.Paired("192.168.1.5", "server-1"), runPairing(link, ops))
        assertEquals(
            listOf("pin 172.20.0.1", "pin 192.168.1.5", "connect 192.168.1.5", "pair 192.168.1.5"),
            ops.calls,
        )
    }

    @Test
    fun whenEveryPinIsRefusedNothingIsContacted() {
        val ops = Fake(reachable = setOf("172.20.0.1", "192.168.1.5"), pinRefused = setOf("172.20.0.1", "192.168.1.5"))
        assertEquals(PairOutcome.Unreachable("trusts a different computer"), runPairing(link, ops))
        assertEquals(listOf("pin 172.20.0.1", "pin 192.168.1.5"), ops.calls)
    }

    @Test
    fun withoutAFingerprintNothingIsPinned() {
        val ops = Fake(reachable = setOf("172.20.0.1"))
        runPairing(link.copy(fingerprint = null), ops)
        assertEquals(listOf("connect 172.20.0.1", "pair 172.20.0.1"), ops.calls)
    }

    /**
     * BC3: with no fingerprint the connection pins whatever answers, and a
     * pairing that failed kept that pin with no card to remove it, so
     * another computer at the address could not be paired. It is taken
     * back, unless a saved computer's pin is there.
     */
    @Test
    fun withoutAFingerprintAFailedPairingLeavesNoPin() {
        val pinless = link.copy(addresses = listOf("192.0.2.1", "192.0.2.2"), fingerprint = null)
        val refused = Fake(reachable = setOf("192.0.2.1"), accept = false)
        assertEquals(PairOutcome.Expired, runPairing(pinless, refused))
        assertEquals(listOf("connect 192.0.2.1", "pair 192.0.2.1", "forget 192.0.2.1"), refused.calls)

        val away = Fake(reachable = emptySet())
        runPairing(pinless, away)
        assertEquals(listOf("connect 192.0.2.1", "forget 192.0.2.1", "connect 192.0.2.2", "forget 192.0.2.2"), away.calls)

        val saved = Fake(reachable = setOf("192.0.2.1"), accept = false, saved = setOf("192.0.2.1"))
        runPairing(pinless, saved)
        assertEquals(false, saved.calls.any { it.startsWith("forget") })
    }
}
