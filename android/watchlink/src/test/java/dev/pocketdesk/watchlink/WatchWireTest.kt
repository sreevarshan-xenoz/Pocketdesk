package dev.pocketdesk.watchlink

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class WatchWireTest {
    private val key = ByteArray(32) { (it * 7).toByte() }
    private val challenge = ByteArray(16) { (it + 1).toByte() }

    @Test
    fun aRequestOpensUnderItsKeyAndChallenge() {
        val frame = WatchWire.sealRequest(key, challenge, WatchOp.Music, 5, WatchBodies.music(MusicCommand.Next))
        val request = WatchWire.openRequest(key, challenge, frame)
        assertNotNull(request)
        assertEquals(WatchOp.Music, request!!.op)
        assertEquals(5L, request.counter)
        assertEquals(MusicCommand.Next, WatchBodies.readMusic(request.body))
    }

    @Test
    fun aRequestOfAnotherKeyOrChallengeDoesNotOpen() {
        val frame = WatchWire.sealRequest(key, challenge, WatchOp.Wake, 1)
        val otherKey = key.copyOf().also { it[0] = (it[0] + 1).toByte() }
        val otherChallenge = challenge.copyOf().also { it[15] = (it[15] + 1).toByte() }
        assertNull(WatchWire.openRequest(otherKey, challenge, frame))
        // A frame recorded in one connection is nothing in the next.
        assertNull(WatchWire.openRequest(key, otherChallenge, frame))
    }

    @Test
    fun aChangedByteDoesNotOpen() {
        val frame = WatchWire.sealRequest(key, challenge, WatchOp.DeckPress, 9, WatchBodies.deckPress(3, "Mute"))
        for (i in frame.indices) {
            val bad = frame.copyOf().also { it[i] = (it[i].toInt() xor 1).toByte() }
            assertNull("byte $i", WatchWire.openRequest(key, challenge, bad))
        }
        assertNull(WatchWire.openRequest(key, challenge, frame.copyOf(frame.size - 1)))
        assertNull(WatchWire.openRequest(key, challenge, ByteArray(0)))
        assertNull(WatchWire.openRequest(key, challenge, ByteArray(WatchWire.MAX_FRAME + 1)))
    }

    @Test
    fun anOpCodeNobodyKnowsDoesNotOpenEvenWithAGoodMac() {
        val head = byteArrayOf(99, 0, 0, 0, 1)
        val mac = WatchWire.hmac(key, "PDW1q".toByteArray(), challenge, head)
        assertNull(WatchWire.openRequest(key, challenge, head + mac))
    }

    @Test
    fun anAnswerOpensForItsCounterOnly() {
        val frame = WatchWire.sealResponse(key, challenge, 4, WatchStatus.Failed, WatchBodies.why("nope"))
        val answer = WatchWire.openResponse(key, challenge, 4, frame)
        assertEquals(WatchStatus.Failed, answer!!.status)
        assertEquals("nope", WatchBodies.readWhy(answer.body))
        assertNull(WatchWire.openResponse(key, challenge, 5, frame))
        assertNull(WatchWire.openResponse(key, challenge.reversedArray(), 4, frame))
    }

    @Test
    fun aRequestIsNotTakenForAnAnswer() {
        val frame = WatchWire.sealRequest(key, challenge, WatchOp.Status, 1)
        assertNull(WatchWire.openResponse(key, challenge, 1, frame))
    }

    @Test
    fun theHelloReadsBack() {
        val hello = WatchWire.readHello(WatchWire.hello(challenge, paired = true))
        assertArrayEquals(challenge, hello!!.challenge)
        assertTrue(hello.paired)
        assertFalse(WatchWire.readHello(WatchWire.hello(challenge, paired = false))!!.paired)
        assertNull(WatchWire.readHello("XXXX".toByteArray() + challenge + byteArrayOf(0)))
        assertNull(WatchWire.readHello(WatchWire.hello(challenge, true).copyOf(10)))
        assertNull(WatchWire.readHello("PDW1".toByteArray() + challenge + byteArrayOf(2)))
    }

    @Test
    fun framesCrossAStreamInOrder() {
        val out = ByteArrayOutputStream()
        WatchWire.write(out, byteArrayOf(1, 2, 3))
        WatchWire.write(out, byteArrayOf(9))
        val input = ByteArrayInputStream(out.toByteArray())
        assertArrayEquals(byteArrayOf(1, 2, 3), WatchWire.read(input))
        assertArrayEquals(byteArrayOf(9), WatchWire.read(input))
        assertNull(WatchWire.read(input))
    }

    @Test
    fun aFrameNeverTrustsItsOwnSize() {
        // A length of 0 and one past the limit are refused before anything is allocated.
        for (size in listOf(0, WatchWire.MAX_FRAME + 1, 0xffff)) {
            val bytes = byteArrayOf((size shr 8).toByte(), size.toByte())
            try {
                WatchWire.read(ByteArrayInputStream(bytes))
                fail("size $size was read")
            } catch (_: IOException) {
            }
        }
        // A frame cut short is an error, not a short frame.
        try {
            WatchWire.read(ByteArrayInputStream(byteArrayOf(0, 5, 1, 2)))
            fail("a cut frame was read")
        } catch (_: IOException) {
        }
    }

    @Test
    fun theBodiesReadBackAndRefuseWhatIsLeftOverOrShort() {
        val status = WatchBodies.Status("Laptop", setOf(WatchButton.Wake, WatchButton.Music), 2)
        val read = WatchBodies.readStatus(WatchBodies.status(status))!!
        assertEquals("Laptop", read.computer)
        assertEquals(status.buttons, read.buttons)
        assertEquals(2, read.asks)
        assertNull(WatchBodies.readStatus(WatchBodies.status(status) + byteArrayOf(0)))
        assertNull(WatchBodies.readStatus(byteArrayOf(5, 65)))

        val deck = WatchBodies.readDeck(WatchBodies.deck(listOf(WatchBodies.Deck(0, "Mute"), WatchBodies.Deck(300, "Lock"))))!!
        assertEquals(listOf(0 to "Mute", 300 to "Lock"), deck.map { it.index to it.label })

        val asks = WatchBodies.readAsks(WatchBodies.asks(listOf(WatchBodies.Ask("a-1", true, "sudo pacman -Syu"))))!!
        assertEquals("a-1", asks[0].id)
        assertTrue(asks[0].approval)
        assertEquals("sudo pacman -Syu", asks[0].text)
        assertEquals("a-1", WatchBodies.readAskDeny(WatchBodies.askDeny("a-1")))
        assertNull(WatchBodies.readMusic(byteArrayOf(7)))
        assertNull(WatchBodies.readMusic(byteArrayOf(0, 0)))
    }

    @Test
    fun aListHoldsAShortListOnly() {
        val many = List(20) { WatchBodies.Deck(it, "b$it") }
        assertEquals(WatchBodies.MAX_DECK, WatchBodies.readDeck(WatchBodies.deck(many))!!.size)
        val asks = List(9) { WatchBodies.Ask("id$it", false, "q") }
        assertEquals(WatchBodies.MAX_ASKS, WatchBodies.readAsks(WatchBodies.asks(asks))!!.size)
        // A body that claims more than the limit is refused.
        assertNull(WatchBodies.readDeck(byteArrayOf((WatchBodies.MAX_DECK + 1).toByte())))
    }

    @Test
    fun buttonsKeepTheirBits() {
        for (set in listOf(emptySet(), WatchButton.DEFAULT, WatchButton.entries.toSet())) {
            assertEquals(set, WatchButton.fromMask(WatchButton.mask(set)))
        }
        assertFalse(WatchButton.Deck in WatchButton.DEFAULT)
        // The bits are the wire's: a change here is a protocol change.
        assertEquals(listOf(1, 2, 4, 8, 16), WatchButton.entries.map { it.bit })
        assertEquals(listOf(1, 2, 3, 4, 5, 6, 7, 8), WatchOp.entries.map { it.code })
    }
}
