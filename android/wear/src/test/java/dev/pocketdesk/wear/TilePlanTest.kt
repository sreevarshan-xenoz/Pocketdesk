package dev.pocketdesk.wear

import dev.pocketdesk.watchlink.MusicCommand
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchOp
import dev.pocketdesk.watchlink.WatchStatus
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TilePlanTest {
    private fun answer(status: WatchStatus, body: ByteArray = ByteArray(0)) = Result.success(WatchWire.Response(status, body))

    @Test
    fun theChipsAreTheButtonsThePhoneHasOn() {
        val all = TilePlan.chips(WatchButton.entries.toSet(), listOf(WatchBodies.Ask("a-1", true, "sudo ls")))
        assertEquals(
            listOf(
                TilePlan.Label.Wake,
                TilePlan.Label.Ring,
                TilePlan.Label.PlayPause,
                TilePlan.Label.Next,
                TilePlan.Label.Deck,
                TilePlan.Label.Deny,
            ),
            all.map { it.label },
        )
        assertTrue(all.single { it.label == TilePlan.Label.Deck }.opensList)
        assertEquals("deny:a-1", all.single { it.label == TilePlan.Label.Deny }.id)
        assertEquals(emptyList<TilePlan.Chip>(), TilePlan.chips(emptySet(), emptyList()))
        // A question with its button off is not offered.
        assertTrue(TilePlan.chips(setOf(WatchButton.Wake), listOf(WatchBodies.Ask("a", false, "q"))).none { it.label == TilePlan.Label.Deny })
        // The default set has no deck chip.
        assertTrue(TilePlan.chips(WatchButton.DEFAULT, emptyList()).none { it.label == TilePlan.Label.Deck })
    }

    @Test
    fun aClickableMapsToItsRequest() {
        assertEquals(TilePlan.Act.Wake, TilePlan.act("wake#0a1b2c3d"))
        assertEquals(TilePlan.Act.Ring, TilePlan.act("ring#0a1b2c3d"))
        assertEquals(TilePlan.Act.Music(MusicCommand.PlayPause), TilePlan.act("play#0a1b2c3d"))
        assertEquals(TilePlan.Act.Music(MusicCommand.Next), TilePlan.act("next#0a1b2c3d"))
        assertEquals(TilePlan.Act.Deny("a-1"), TilePlan.act("deny:a-1#0a1b2c3d"))
        assertEquals(WatchOp.AskDeny, TilePlan.act("deny:a-1#0a1b2c3d")!!.op)
        assertEquals(WatchOp.Music, TilePlan.act("play#0a1b2c3d")!!.op)
    }

    @Test
    fun aStaleOrForeignIdAsksForNothing() {
        assertNull(TilePlan.act(null))
        assertNull(TilePlan.act(""))
        assertNull(TilePlan.act("deck#0a1b2c3d"))
        assertNull(TilePlan.act("deny:#0a1b2c3d"))
        assertNull(TilePlan.act("deny:../../x#0a1b2c3d"))
        assertNull(TilePlan.act("deny:" + "a".repeat(65) + "#0a1b2c3d"))
        assertNull(TilePlan.act("WAKE#0a1b2c3d"))
        // An id with no nonce, or a bad one, is not one this drawing made.
        assertNull(TilePlan.act("wake"))
        assertNull(TilePlan.act("wake#"))
        assertNull(TilePlan.act("wake#xyz"))
        assertNull(TilePlan.act("wake#0a1b2c3d4"))
    }

    @Test
    fun aDenyNamesTheQuestionNotItsPlace() {
        // The list may change between the draw and the tap: the id is the question's own.
        val first = TilePlan.chips(setOf(WatchButton.Asks), listOf(WatchBodies.Ask("old-1", false, "q")))
        val second = TilePlan.chips(setOf(WatchButton.Asks), listOf(WatchBodies.Ask("new-2", false, "q")))
        assertEquals("deny:old-1", first.single().id)
        assertEquals("deny:new-2", second.single().id)
    }

    @Test
    fun anAnswerIsCalledWhatItWas() {
        val wake = TilePlan.Act.Wake
        assertEquals(TilePlan.Line.Sent, TilePlan.line(wake, answer(WatchStatus.Ok)))
        assertEquals(TilePlan.Line.Answered, TilePlan.line(TilePlan.Act.Deny("a"), answer(WatchStatus.Ok)))
        assertEquals(TilePlan.Line.Off, TilePlan.line(wake, answer(WatchStatus.Refused)))
        assertEquals(TilePlan.Line.Busy, TilePlan.line(wake, answer(WatchStatus.Busy)))
        assertEquals(TilePlan.Line.Why("away from its network"), TilePlan.line(wake, answer(WatchStatus.Failed, WatchBodies.why("away from its network"))))
        assertEquals(TilePlan.Line.Failed, TilePlan.line(wake, answer(WatchStatus.Failed, byteArrayOf(9))))
        assertEquals(TilePlan.Line.Nothing, TilePlan.line(null, answer(WatchStatus.Ok)))
    }

    @Test
    fun noAnswerIsUnreachableForALinkAndFailedForAnythingElse() {
        assertEquals(TilePlan.Line.Unreachable, TilePlan.line(TilePlan.Act.Ring, Result.failure(IOException("timed out"))))
        assertEquals(TilePlan.Line.Failed, TilePlan.line(TilePlan.Act.Ring, Result.failure(IllegalStateException("x"))))
    }

    @Test
    fun aConnectionThatNeverCameToBeIsNamedForWhy() {
        assertEquals(TilePlan.Line.NotPaired, Phone.failed(NotPairedException()))
        assertEquals(TilePlan.Line.NoBluetooth, Phone.failed(NoBluetoothException()))
        assertEquals(TilePlan.Line.Unreachable, Phone.failed(IOException("no route")))
        assertEquals(TilePlan.Line.Unreachable, Phone.failed(null))
    }

    @Test
    fun aTapIsActedOnOnceWhateverHowOftenTheTileIsShownAgain() {
        var last: String? = null
        val guard = TilePlan.ClickGuard({ last }, { last = it })
        val tap = TilePlan.clickId(TilePlan.WAKE, "0a1b2c3d")
        assertTrue(guard.fresh(tap))
        // The watch hands the same id back each time the tile is shown.
        repeat(3) { assertFalse(guard.fresh(tap)) }
        // Nothing clicked is not a tap.
        assertFalse(guard.fresh(null))
        assertFalse(guard.fresh(""))
        // The next drawing's tap on the same button is a new one.
        assertTrue(guard.fresh(TilePlan.clickId(TilePlan.WAKE, "11223344")))
        assertEquals(TilePlan.Act.Wake, TilePlan.act(TilePlan.clickId(TilePlan.WAKE, "11223344")))
    }

    @Test
    fun aGuardKeptBetweenProcessesStillRefusesTheLastTap() {
        var stored: String? = null
        val before = TilePlan.ClickGuard({ stored }, { stored = it })
        val tap = TilePlan.clickId(TilePlan.RING, "deadbeef")
        assertTrue(before.fresh(tap))
        // A new process: a new guard over the same storage.
        val after = TilePlan.ClickGuard({ stored }, { stored = it })
        assertFalse(after.fresh(tap))
    }

    @Test
    fun aNonceIsEightHexDigitsAndDiffersEachDrawing() {
        val a = TilePlan.nonce()
        val b = TilePlan.nonce()
        assertTrue(Regex("^[0-9a-f]{8}$").matches(a))
        assertTrue(a != b)
    }
}
