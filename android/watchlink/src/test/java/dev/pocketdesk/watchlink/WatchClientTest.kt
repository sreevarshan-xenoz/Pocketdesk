package dev.pocketdesk.watchlink

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** The watch's client against the phone's session, over in-memory streams: nothing real is touched. */
class WatchClientTest {
    private val key = WatchPairing.newKey()
    private val challenge = WatchPairing.newChallenge()

    /** A phone: says hello, then answers each frame the client writes with [reply]. */
    private class Phone(hello: ByteArray, private val reply: (ByteArray) -> ByteArray?) {
        private val toClient = java.util.ArrayDeque<Byte>()
        private val pending = ByteArrayOutputStream()

        init {
            queue(hello)
        }

        private fun queue(frame: ByteArray) {
            toClient.add((frame.size shr 8).toByte())
            toClient.add(frame.size.toByte())
            frame.forEach { toClient.add(it) }
        }

        val input = object : InputStream() {
            override fun read(): Int = toClient.pollFirst()?.toInt()?.and(0xff) ?: -1
        }

        val output = object : OutputStream() {
            override fun write(b: Int) {
                pending.write(b)
                val bytes = pending.toByteArray()
                if (bytes.size < 2) return
                val size = ((bytes[0].toInt() and 0xff) shl 8) or (bytes[1].toInt() and 0xff)
                if (bytes.size < 2 + size) return
                pending.reset()
                reply(bytes.copyOfRange(2, 2 + size))?.let(::queue)
            }
        }
    }

    private class Recorder : WatchActions {
        val calls = mutableListOf<String>()
        override fun status() = WatchBodies.Status("Laptop", emptySet(), 1)
        override fun wake(): Result<Unit> {
            calls += "wake"
            return Result.success(Unit)
        }

        override fun ring(): Result<Unit> = Result.success(Unit)
        override fun music(command: MusicCommand): Result<Unit> {
            calls += command.command
            return Result.success(Unit)
        }

        override fun deckList() = Result.success(listOf(WatchBodies.Deck(0, "Mute")))
        override fun deckPress(index: Int, label: String) = Result.success(Unit)
        override fun asks() = listOf(WatchBodies.Ask("a1", true, "sudo ls"))
        override fun deny(id: String): Result<Unit> {
            calls += "deny:$id"
            return Result.success(Unit)
        }
    }

    private fun phone(actions: WatchActions, on: Set<WatchButton>, held: ByteArray = key): Phone {
        val session = WatchSession(held, challenge, actions, { on }, WatchLimits { 0L })
        return Phone(WatchWire.hello(challenge, paired = true)) { session.serve(it) }
    }

    @Test
    fun aWatchAsksAndTheAnswersVerify() {
        val actions = Recorder()
        val p = phone(actions, WatchButton.entries.toSet())
        val client = WatchClient(p.input, p.output, key)
        assertTrue(client.open())
        val status = WatchBodies.readStatus(client.call(WatchOp.Status).body)!!
        assertEquals("Laptop", status.computer)
        assertEquals(WatchButton.entries.toSet(), status.buttons)
        assertEquals(WatchStatus.Ok, client.call(WatchOp.Wake).status)
        assertEquals(WatchStatus.Ok, client.call(WatchOp.Music, WatchBodies.music(MusicCommand.Next)).status)
        val asks = WatchBodies.readAsks(client.call(WatchOp.AskList).body)!!
        assertEquals("a1", asks[0].id)
        assertEquals(WatchStatus.Ok, client.call(WatchOp.AskDeny, WatchBodies.askDeny("a1")).status)
        assertEquals(listOf("wake", "next", "deny:a1"), actions.calls)
    }

    @Test
    fun aButtonThatIsOffComesBackRefused() {
        val actions = Recorder()
        val p = phone(actions, setOf(WatchButton.Music))
        val client = WatchClient(p.input, p.output, key)
        client.open()
        assertEquals(WatchStatus.Refused, client.call(WatchOp.Wake).status)
        assertTrue(actions.calls.isEmpty())
    }

    @Test
    fun aPhoneThatHoldsAnotherKeyIsNeverBelieved() {
        // An impostor service answers nothing the watch would accept: its frames do not verify.
        val p = phone(Recorder(), WatchButton.entries.toSet(), held = ByteArray(32) { 5 })
        val client = WatchClient(p.input, p.output, key)
        client.open()
        // The impostor drops the watch's frame (it cannot verify it), so there is no answer.
        try {
            client.call(WatchOp.Status)
            fail("an answer was believed")
        } catch (_: IOException) {
        }
    }

    @Test
    fun aServiceThatIsNotThePhonesIsRefusedAtTheHello() {
        val p = Phone(byteArrayOf(1, 2, 3)) { null }
        try {
            WatchClient(p.input, p.output, key).open()
            fail("a hello that is not one was taken")
        } catch (_: IOException) {
        }
        // Nothing at all.
        val silent = object : InputStream() {
            override fun read(): Int = -1
        }
        try {
            WatchClient(silent, ByteArrayOutputStream(), key).open()
            fail("a silence was taken")
        } catch (_: IOException) {
        }
    }

    @Test
    fun pairingHandsTheKeyOverOnlyForTheRightCode() {
        // The phone's side of pairing, as the server runs it.
        fun serve(typed: String): Phone {
            val window = PairingWindow({ 0L })
            window.open()
            window.offer(typed)
            return Phone(WatchWire.hello(challenge, paired = false)) { frame ->
                val pair = WatchPairing.readPairFrame(frame)!!
                val code = window.typed()!!
                val right = WatchWire.same(WatchPairing.proof(code, challenge, pair.name, pair.publicKey), pair.proof)
                window.judged("watch", right)
                if (right) {
                    val mine = WatchPairing.newEphemeral()
                    val secret = WatchPairing.agree(mine, pair.publicKey)!!
                    WatchPairing.accepted(mine.public.encoded, WatchPairing.mask(code, challenge, secret, key))
                } else {
                    WatchPairing.refused()
                }
            }
        }

        val good = serve("123456")
        val c = WatchClient(good.input, good.output, null)
        assertFalse(c.open())
        assertArrayEquals(key, c.pair("123456", "Test watch"))

        val bad = serve("654321")
        val d = WatchClient(bad.input, bad.output, null)
        d.open()
        try {
            d.pair("123456", "Test watch")
            fail("a wrong code paired")
        } catch (_: IOException) {
        }
    }
}
