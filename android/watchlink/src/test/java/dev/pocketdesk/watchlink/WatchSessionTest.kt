package dev.pocketdesk.watchlink

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WatchSessionTest {
    private val key = ByteArray(32) { (it + 3).toByte() }
    private val challenge = ByteArray(16) { (it * 3).toByte() }

    /** The phone's calls, recorded; none of it touches a real phone, computer or speaker. */
    private class Fake : WatchActions {
        val calls = mutableListOf<String>()
        var fail = false
        var plain = true
        var asksWaiting = listOf(WatchBodies.Ask("ask-1", true, "sudo ls"))
        var denied = mutableSetOf<String>()

        private fun result(name: String): Result<Unit> {
            calls += name
            return if (fail) Result.failure(if (plain) WatchFailure("not reachable") else IllegalStateException("its address is 192.0.2.7")) else Result.success(Unit)
        }

        override fun status() = WatchBodies.Status("Laptop", WatchButton.entries.toSet(), asksWaiting.size)
        override fun wake() = result("wake")
        override fun ring() = result("ring")
        override fun music(command: MusicCommand) = result("music:${command.command}")
        override fun deckList(): Result<List<WatchBodies.Deck>> {
            calls += "decklist"
            return Result.success(listOf(WatchBodies.Deck(2, "Mute")))
        }

        override fun deckPress(index: Int, label: String) = result("deck:$index:$label")
        override fun asks() = asksWaiting
        override fun deny(id: String): Result<Unit> {
            calls += "deny:$id"
            return if (id in asksWaiting.map { it.id }) Result.success(Unit) else Result.failure(WatchFailure("not waiting"))
        }
    }

    private var now = 1_000_000L
    private val limits = WatchLimits { now }

    private fun session(actions: WatchActions, on: Set<WatchButton> = WatchButton.entries.toSet()) =
        WatchSession(key, challenge, actions, { on }, limits)

    private var counter = 0L

    private fun ask(session: WatchSession, op: WatchOp, body: ByteArray = ByteArray(0), at: Long = ++counter): WatchWire.Response? {
        val reply = session.serve(WatchWire.sealRequest(key, challenge, op, at, body)) ?: return null
        return WatchWire.openResponse(key, challenge, at, reply)
    }

    @Test
    fun aButtonThatIsOnRunsAndAnswersOk() {
        val fake = Fake()
        val s = session(fake)
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.Wake)!!.status)
        now += 70_000
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.Ring)!!.status)
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.Music, WatchBodies.music(MusicCommand.PlayPause))!!.status)
        assertEquals(listOf("wake", "ring", "music:playPause"), fake.calls)
    }

    @Test
    fun aButtonThatIsOffIsRefusedAndNothingRuns() {
        val fake = Fake()
        val s = session(fake, on = setOf(WatchButton.Music))
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.Wake)!!.status)
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.Ring)!!.status)
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.DeckPress, WatchBodies.deckPress(1, "x"))!!.status)
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.DeckList)!!.status)
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.AskList)!!.status)
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.AskDeny, WatchBodies.askDeny("ask-1"))!!.status)
        assertTrue(fake.calls.isEmpty())
    }

    @Test
    fun theSwitchesAreReadAtEachRequestNotWhenTheConnectionOpened() {
        var on = setOf(WatchButton.Wake)
        val fake = Fake()
        val s = WatchSession(key, challenge, fake, { on }, limits)
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.Wake)!!.status)
        on = emptySet()
        now += 20_000
        assertEquals(WatchStatus.Refused, ask(s, WatchOp.Wake)!!.status)
    }

    @Test
    fun statusSaysTheButtonsThatAreOnNow() {
        val s = session(Fake(), on = setOf(WatchButton.Music, WatchButton.Asks))
        val status = WatchBodies.readStatus(ask(s, WatchOp.Status)!!.body)!!
        assertEquals(setOf(WatchButton.Music, WatchButton.Asks), status.buttons)
        assertEquals("Laptop", status.computer)
        assertEquals(1, status.asks)
    }

    @Test
    fun aReplayedOrOldCounterIsDroppedWithoutAnAnswer() {
        val fake = Fake()
        val s = session(fake)
        val frame = WatchWire.sealRequest(key, challenge, WatchOp.Music, 7, WatchBodies.music(MusicCommand.Next))
        assertNotNull(s.serve(frame))
        now += 1_000
        // The same bytes again: heard and dropped.
        assertNull(s.serve(frame))
        // A lower counter, validly made, is no newer.
        assertNull(s.serve(WatchWire.sealRequest(key, challenge, WatchOp.Music, 6, WatchBodies.music(MusicCommand.Next))))
        assertEquals(listOf("music:next"), fake.calls)
    }

    @Test
    fun aFrameFromAnotherKeyIsDroppedAndThreeEndTheConnection() {
        val s = session(Fake())
        val other = ByteArray(32) { 1 }
        repeat(WatchSession.BAD_LIMIT) {
            assertFalse(s.dropped)
            assertNull(s.serve(WatchWire.sealRequest(other, challenge, WatchOp.Wake, 1)))
        }
        assertTrue(s.dropped)
    }

    @Test
    fun aGoodFrameClearsTheCountOfBadOnes() {
        val s = session(Fake())
        assertNull(s.serve(ByteArray(40)))
        assertNull(s.serve(ByteArray(40)))
        assertNotNull(ask(s, WatchOp.Status))
        assertNull(s.serve(ByteArray(40)))
        assertFalse(s.dropped)
    }

    @Test
    fun aRingIsOnceAMinuteAndTheLimitSurvivesAReconnect() {
        val fake = Fake()
        assertEquals(WatchStatus.Ok, ask(session(fake), WatchOp.Ring)!!.status)
        now += 5_000
        // A new connection, as a watch makes between taps: the limit is the phone's.
        counter = 0
        assertEquals(WatchStatus.Busy, ask(session(fake), WatchOp.Ring)!!.status)
        now += 60_000
        counter = 0
        assertEquals(WatchStatus.Ok, ask(session(fake), WatchOp.Ring)!!.status)
        assertEquals(listOf("ring", "ring"), fake.calls)
    }

    @Test
    fun aDeckPressTooSoonIsBusy() {
        val fake = Fake()
        val s = session(fake)
        val press = WatchBodies.deckPress(2, "Mute")
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.DeckPress, press)!!.status)
        assertEquals(WatchStatus.Busy, ask(s, WatchOp.DeckPress, press)!!.status)
        now += 2_000
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.DeckPress, press)!!.status)
        assertEquals(listOf("deck:2:Mute", "deck:2:Mute"), fake.calls)
    }

    @Test
    fun aFailureSaysTheWordsMadeForTheWatchAndNeverAnExceptionsText() {
        val plain = Fake().apply { fail = true }
        val answer = ask(session(plain), WatchOp.Wake)!!
        assertEquals(WatchStatus.Failed, answer.status)
        assertEquals("not reachable", WatchBodies.readWhy(answer.body))
        now += 20_000
        val stray = Fake().apply {
            fail = true
            this.plain = false
        }
        val other = ask(session(stray), WatchOp.Wake)!!
        assertEquals("did not work", WatchBodies.readWhy(other.body))
    }

    @Test
    fun anExceptionFromThePhoneIsAFailureWithNoText() {
        val boom = object : WatchActions by Fake() {
            override fun wake(): Result<Unit> = throw IllegalStateException("/data/user/0/secret")
        }
        val answer = ask(session(boom), WatchOp.Wake)!!
        assertEquals(WatchStatus.Failed, answer.status)
        assertEquals("did not work", WatchBodies.readWhy(answer.body))
    }

    @Test
    fun aBodyThatDoesNotReadIsAFailureNotACrash() {
        val fake = Fake()
        val s = session(fake)
        assertEquals(WatchStatus.Failed, ask(s, WatchOp.Music, byteArrayOf(9))!!.status)
        assertEquals(WatchStatus.Failed, ask(s, WatchOp.DeckPress, byteArrayOf(1))!!.status)
        assertEquals(WatchStatus.Failed, ask(s, WatchOp.AskDeny, byteArrayOf(5, 65))!!.status)
        assertTrue(fake.calls.isEmpty())
    }

    @Test
    fun aWatchCanSayNoToAnAskButHasNoWayToSayYes() {
        val fake = Fake()
        val s = session(fake)
        val list = WatchBodies.readAsks(ask(s, WatchOp.AskList)!!.body)!!
        assertEquals("ask-1", list[0].id)
        assertTrue(list[0].approval)
        assertEquals(WatchStatus.Ok, ask(s, WatchOp.AskDeny, WatchBodies.askDeny("ask-1"))!!.status)
        now += 1_000
        // An id that is not waiting fails: the phone never answers a question it does not hold.
        assertEquals(WatchStatus.Failed, ask(s, WatchOp.AskDeny, WatchBodies.askDeny("other"))!!.status)
        assertEquals(listOf("deny:ask-1", "deny:other"), fake.calls)
        // The protocol has no op that answers Yes: every op is one of these.
        assertEquals(
            setOf("Status", "Wake", "Ring", "Music", "DeckList", "DeckPress", "AskList", "AskDeny"),
            WatchOp.entries.map { it.name }.toSet(),
        )
    }
}
