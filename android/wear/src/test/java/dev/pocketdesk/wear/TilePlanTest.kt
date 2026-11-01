package dev.pocketdesk.wear

import dev.pocketdesk.watchlink.MusicCommand
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchOp
import dev.pocketdesk.watchlink.WatchStatus
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException
import org.junit.Assert.assertEquals
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
        assertEquals(TilePlan.Act.Wake, TilePlan.act("wake"))
        assertEquals(TilePlan.Act.Ring, TilePlan.act("ring"))
        assertEquals(TilePlan.Act.Music(MusicCommand.PlayPause), TilePlan.act("play"))
        assertEquals(TilePlan.Act.Music(MusicCommand.Next), TilePlan.act("next"))
        assertEquals(TilePlan.Act.Deny("a-1"), TilePlan.act("deny:a-1"))
        assertEquals(WatchOp.AskDeny, TilePlan.act("deny:a-1")!!.op)
        assertEquals(WatchOp.Music, TilePlan.act("play")!!.op)
    }

    @Test
    fun aStaleOrForeignIdAsksForNothing() {
        assertNull(TilePlan.act(null))
        assertNull(TilePlan.act(""))
        assertNull(TilePlan.act("deck"))
        assertNull(TilePlan.act("deny:"))
        assertNull(TilePlan.act("deny:../../x"))
        assertNull(TilePlan.act("deny:" + "a".repeat(65)))
        assertNull(TilePlan.act("WAKE"))
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
}
