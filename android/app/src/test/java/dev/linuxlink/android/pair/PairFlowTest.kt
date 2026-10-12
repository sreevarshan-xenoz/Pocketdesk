package dev.linuxlink.android.pair

import org.junit.Assert.assertEquals
import org.junit.Test

class PairFlowTest {
    private val link = PairLink(
        name = "Desk", deviceId = "d-1",
        addresses = listOf("172.20.0.1", "192.168.1.5"),
        controlPort = 1716, streamPort = 4716, pin = "123456",
        fingerprint = "ab".repeat(32), endpoint = null,
    )

    /** Records calls; [reachable] answers, [accept] decides the PIN. */
    private class Fake(val reachable: Set<String>, val accept: Boolean = true) : PairOps {
        val calls = mutableListOf<String>()
        override fun pinCertificate(address: String, fingerprint: String): Result<Unit> {
            calls += "pin $address"
            return Result.success(Unit)
        }
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
            listOf("pin 172.20.0.1", "connect 172.20.0.1", "pin 192.168.1.5", "connect 192.168.1.5", "pair 192.168.1.5"),
            ops.calls,
        )
    }

    @Test
    fun aRejectedPinIsAnExpiredCodeAndStopsTrying() {
        val ops = Fake(reachable = setOf("172.20.0.1", "192.168.1.5"), accept = false)
        assertEquals(PairOutcome.Expired, runPairing(link, ops))
        assertEquals(listOf("pin 172.20.0.1", "connect 172.20.0.1", "pair 172.20.0.1"), ops.calls)
    }

    @Test
    fun noReachableAddressIsUnreachable() {
        val outcome = runPairing(link, Fake(reachable = emptySet()))
        assertEquals(PairOutcome.Unreachable("timeout"), outcome)
    }

    @Test
    fun withoutAFingerprintNothingIsPinned() {
        val ops = Fake(reachable = setOf("172.20.0.1"))
        runPairing(link.copy(fingerprint = null), ops)
        assertEquals(listOf("connect 172.20.0.1", "pair 172.20.0.1"), ops.calls)
    }
}
