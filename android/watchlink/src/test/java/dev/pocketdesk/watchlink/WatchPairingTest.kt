package dev.pocketdesk.watchlink

import java.security.SecureRandom
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WatchPairingTest {
    private val challenge = ByteArray(16) { (it + 9).toByte() }

    @Test
    fun aCodeIsSixDigitsAndTypedWithOrWithoutSpaces() {
        repeat(50) { assertTrue(WatchPairing.isCode(WatchPairing.newCode(SecureRandom()))) }
        assertTrue(WatchPairing.isCode("123 456"))
        assertEquals("123456", WatchPairing.normalize("123 456"))
        assertFalse(WatchPairing.isCode("12345"))
        assertFalse(WatchPairing.isCode("1234567"))
        assertFalse(WatchPairing.isCode("12345a"))
        assertFalse(WatchPairing.isCode(""))
    }

    @Test
    fun theProofFitsOnlyItsCodeChallengeAndName() {
        val proof = WatchPairing.proof("123456", challenge, "Watch")
        assertArrayEquals(proof, WatchPairing.proof("123456", challenge, "Watch"))
        assertFalse(proof.contentEquals(WatchPairing.proof("123457", challenge, "Watch")))
        assertFalse(proof.contentEquals(WatchPairing.proof("123456", challenge.reversedArray(), "Watch")))
        assertFalse(proof.contentEquals(WatchPairing.proof("123456", challenge, "Other")))
    }

    @Test
    fun theKeyGoesMaskedAndComesBackWithTheRightCodeOnly() {
        val key = WatchPairing.newKey()
        val masked = WatchPairing.mask("123456", challenge, key)
        assertFalse(masked.contentEquals(key))
        assertArrayEquals(key, WatchPairing.readAnswer("123456", challenge, WatchPairing.accepted(masked)))
        // A wrong code unmasks to some other bytes: never the key.
        val wrong = WatchPairing.readAnswer("000000", challenge, WatchPairing.accepted(masked))
        assertFalse(key.contentEquals(wrong))
        assertNull(WatchPairing.readAnswer("123456", challenge, WatchPairing.refused()))
        assertNull(WatchPairing.readAnswer("123456", challenge, byteArrayOf(0, 1, 2)))
    }

    @Test
    fun thePairFrameReadsBackAndRefusesGarbage() {
        val frame = WatchPairing.pairFrame("123456", challenge, "My watch")
        val read = WatchPairing.readPairFrame(frame)!!
        assertEquals("My watch", read.name)
        assertArrayEquals(WatchPairing.proof("123456", challenge, "My watch"), read.proof)
        assertNull(WatchPairing.readPairFrame(frame + byteArrayOf(1)))
        assertNull(WatchPairing.readPairFrame(frame.copyOf(10)))
        assertNull(WatchPairing.readPairFrame(byteArrayOf(2) + frame.copyOfRange(1, frame.size)))
        assertNull(WatchPairing.readPairFrame(ByteArray(0)))
        // A request is never a pair frame, and a pair frame never a request.
        val request = WatchWire.sealRequest(ByteArray(32), challenge, WatchOp.Status, 1)
        assertNull(WatchPairing.readPairFrame(request))
        assertNull(WatchWire.openRequest(ByteArray(32), challenge, frame))
    }

    @Test
    fun aWatchNameIsCleanedAndKept() {
        val read = WatchPairing.readPairFrame(WatchPairing.pairFrame("123456", challenge, "Pixel\nWatch‮!"))!!
        assertEquals("Pixel Watch!", read.name)
        assertEquals("watch", WatchPairing.readPairFrame(WatchPairing.pairFrame("123456", challenge, "  "))!!.name)
    }

    @Test
    fun theWindowOpensTakesACodeAndClosesAfterThreeWrong() {
        var now = 0L
        val window = PairingWindow({ now })
        assertFalse(window.isOpen)
        assertFalse(window.offer("123456"))
        window.open()
        assertTrue(window.isOpen)
        assertFalse(window.offer("12"))
        assertNull(window.typed())
        assertTrue(window.offer("123 456"))
        assertEquals("123456", window.typed())
        window.judged(right = false)
        assertNull(window.typed())
        assertTrue(window.offer("111111"))
        window.judged(right = false)
        assertTrue(window.isOpen)
        assertTrue(window.offer("222222"))
        window.judged(right = false)
        // The third wrong code shut it.
        assertFalse(window.isOpen)
        assertFalse(window.offer("333333"))
    }

    @Test
    fun theWindowEndsWithARightCodeAndWithTime() {
        var now = 0L
        val window = PairingWindow({ now })
        window.open()
        assertTrue(window.offer("123456"))
        window.judged(right = true)
        assertFalse(window.isOpen)

        window.open()
        now += PairingWindow.LIFE_MS - 1
        assertTrue(window.isOpen)
        now += 1
        assertFalse(window.isOpen)
        assertFalse(window.offer("123456"))
        assertNotNull(PairingWindow({ 0 }))
    }

    @Test
    fun openingAgainStartsTheCountOver() {
        val window = PairingWindow({ 0 })
        window.open()
        repeat(PairingWindow.MAX_WRONG) {
            window.offer("123456")
            window.judged(false)
        }
        assertFalse(window.isOpen)
        window.open()
        assertTrue(window.isOpen)
        window.offer("123456")
        window.judged(false)
        assertTrue(window.isOpen)
    }
}
