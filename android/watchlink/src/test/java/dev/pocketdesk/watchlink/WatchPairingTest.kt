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
    private val watchKeys = WatchPairing.newEphemeral()
    private val phoneKeys = WatchPairing.newEphemeral()

    /** The phone's answer to a good pair frame, as the server builds it. */
    private fun answer(code: String, key: ByteArray, watchPublic: ByteArray): ByteArray {
        val secret = WatchPairing.agree(phoneKeys, watchPublic)!!
        return WatchPairing.accepted(phoneKeys.public.encoded, WatchPairing.mask(code, challenge, secret, key))
    }

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
    fun theProofFitsOnlyItsCodeChallengeNameAndKey() {
        val pub = watchKeys.public.encoded
        val proof = WatchPairing.proof("123456", challenge, "Watch", pub)
        assertArrayEquals(proof, WatchPairing.proof("123456", challenge, "Watch", pub))
        assertFalse(proof.contentEquals(WatchPairing.proof("123457", challenge, "Watch", pub)))
        assertFalse(proof.contentEquals(WatchPairing.proof("123456", challenge.reversedArray(), "Watch", pub)))
        assertFalse(proof.contentEquals(WatchPairing.proof("123456", challenge, "Other", pub)))
        assertFalse(proof.contentEquals(WatchPairing.proof("123456", challenge, "Watch", phoneKeys.public.encoded)))
    }

    @Test
    fun theKeyComesBackOnlyWithTheRightCode() {
        val key = WatchPairing.newKey()
        val frame = answer("123456", key, watchKeys.public.encoded)
        assertArrayEquals(key, WatchPairing.readAnswer("123456", challenge, watchKeys, frame))
        // A wrong code unmasks to some other bytes: never the key.
        assertFalse(key.contentEquals(WatchPairing.readAnswer("000000", challenge, watchKeys, frame)))
        assertNull(WatchPairing.readAnswer("123456", challenge, watchKeys, WatchPairing.refused()))
        assertNull(WatchPairing.readAnswer("123456", challenge, watchKeys, byteArrayOf(0, 1, 2)))
        assertNull(WatchPairing.readAnswer("123456", challenge, watchKeys, ByteArray(0)))
    }

    @Test
    fun theDigitsAloneDoNotOpenARecordedExchange() {
        // Someone who recorded the whole exchange and guessed the digits still lacks the ECDH secret:
        // the pad is made from the digits and the secret, so the digits alone give another key.
        val key = WatchPairing.newKey()
        val recorded = answer("123456", key, watchKeys.public.encoded)
        val eavesdropper = WatchPairing.newEphemeral()
        val guess = WatchPairing.readAnswer("123456", challenge, eavesdropper, recorded)
        assertFalse(key.contentEquals(guess))
    }

    @Test
    fun aPublicKeyThatIsNotOneIsNeverAgreedWith() {
        assertNull(WatchPairing.agree(watchKeys, byteArrayOf(1, 2, 3)))
        assertNull(WatchPairing.agree(watchKeys, ByteArray(0)))
        val ab = WatchPairing.agree(watchKeys, phoneKeys.public.encoded)
        val ba = WatchPairing.agree(phoneKeys, watchKeys.public.encoded)
        assertArrayEquals(ab, ba)
    }

    @Test
    fun thePairFrameReadsBackAndRefusesGarbage() {
        val frame = WatchPairing.pairFrame("123456", challenge, "My watch", watchKeys)
        val read = WatchPairing.readPairFrame(frame)!!
        assertEquals("My watch", read.name)
        assertArrayEquals(watchKeys.public.encoded, read.publicKey)
        assertArrayEquals(WatchPairing.proof("123456", challenge, "My watch", read.publicKey), read.proof)
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
        val read = WatchPairing.readPairFrame(WatchPairing.pairFrame("123456", challenge, "Pixel\nWatch‮!", watchKeys))!!
        assertEquals("Pixel Watch!", read.name)
        assertEquals("watch", WatchPairing.readPairFrame(WatchPairing.pairFrame("123456", challenge, "  ", watchKeys))!!.name)
    }

    @Test
    fun theWindowOpensAndTakesACode() {
        val window = PairingWindow({ 0L })
        assertFalse(window.isOpen)
        assertFalse(window.offer("123456"))
        window.open()
        assertTrue(window.isOpen)
        assertFalse(window.offer("12"))
        assertNull(window.typed())
        assertTrue(window.offer("123 456"))
        assertEquals("123456", window.typed())
        // The code stays for every connection to judge its own proof against.
        assertEquals("123456", window.typed())
    }

    @Test
    fun aWrongCodeCountsAgainstItsOwnAddressOnly() {
        val window = PairingWindow({ 0L })
        window.open()
        window.offer("123456")
        // A rogue bonded device uses up its own three tries ...
        repeat(PairingWindow.MAX_WRONG) {
            assertTrue(window.enter("rogue"))
            window.judged("rogue", right = false)
            window.release("rogue")
        }
        assertFalse(window.enter("rogue"))
        // ... and the owner's watch still has all of its own, and the window is still open.
        assertTrue(window.isOpen)
        assertTrue(window.enter("watch"))
        window.judged("watch", right = true)
        assertFalse(window.isOpen)
        assertNull(window.typed())
    }

    @Test
    fun oneConnectionPerAddressWaitsAtATime() {
        val window = PairingWindow({ 0L })
        window.open()
        assertTrue(window.enter("a"))
        assertFalse(window.enter("a"))
        assertTrue(window.enter("b"))
        window.release("a")
        assertTrue(window.enter("a"))
    }

    @Test
    fun theWindowClosesAfterTenWrongInAllAndWithTime() {
        var now = 0L
        val window = PairingWindow({ now })
        window.open()
        for (i in 0 until PairingWindow.MAX_WRONG_TOTAL) {
            assertTrue(window.isOpen)
            window.judged("addr$i", right = false)
        }
        assertFalse(window.isOpen)
        assertFalse(window.offer("123456"))
        assertFalse(window.enter("fresh"))

        window.open()
        assertTrue(window.isOpen)
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
        repeat(PairingWindow.MAX_WRONG) { window.judged("a", false) }
        assertFalse(window.enter("a"))
        window.open()
        assertTrue(window.enter("a"))
    }
}
