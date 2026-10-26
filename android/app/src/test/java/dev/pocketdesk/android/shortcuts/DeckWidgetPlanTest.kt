package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.Deck
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DeckWidgetPlanTest {
    private fun button(i: Int) = Deck.Button(i, "B$i", "keys")

    @Test
    fun theFirstSixButtonsFillTheSlotsInOrder() {
        val slots = DeckWidgetPlan.slots((0 until 8).map { button(it) })
        assertEquals(6, slots.size)
        assertEquals(listOf(0, 1, 2, 3, 4, 5), slots.map { it?.index })
        val few = DeckWidgetPlan.slots(listOf(button(4), button(9)))
        assertEquals(6, few.size)
        assertEquals(4, few[0]?.index)
        assertEquals(9, few[1]?.index)
        assertNull(few[2])
        assertTrue(DeckWidgetPlan.slots(emptyList()).all { it == null })
    }

    @Test
    fun theListIsAskedForWhenAskedOrNeverTried() {
        assertTrue(DeckWidgetPlan.askList(DeckWidgetPlan.Why.Refresh, tried = true))
        assertTrue(DeckWidgetPlan.askList(DeckWidgetPlan.Why.Follow, tried = false))
        assertFalse(DeckWidgetPlan.askList(DeckWidgetPlan.Why.Follow, tried = true))
        assertFalse(DeckWidgetPlan.askList(DeckWidgetPlan.Why.Period, tried = true))
        assertTrue(DeckWidgetPlan.askList(DeckWidgetPlan.Why.Period, tried = false))
    }

    @Test
    fun aPressSaysHowItWent() {
        assertEquals(DeckWidgetPlan.Said.Done, DeckWidgetPlan.said(Deck.Outcome(ok = true, reason = null)))
        assertEquals(DeckWidgetPlan.Said.Waiting, DeckWidgetPlan.said(Deck.Outcome(ok = true, reason = null, waiting = true)))
        assertEquals(DeckWidgetPlan.Said.Refused, DeckWidgetPlan.said(Deck.Outcome(ok = false, reason = "Input is off")))
        assertEquals(DeckWidgetPlan.Said.Failed, DeckWidgetPlan.said(null))
    }

    @Test
    fun aListIsKeptOnlyForItsComputer() {
        assertTrue(DeckWidgetPlan.keeps(listFor = "a", lastComputer = "a"))
        assertFalse(DeckWidgetPlan.keeps(listFor = "a", lastComputer = "b"))
        assertFalse(DeckWidgetPlan.keeps(listFor = null, lastComputer = "a"))
        assertFalse(DeckWidgetPlan.keeps(listFor = "a", lastComputer = null))
    }

    /**
     * AZ2: a paired computer's list is kept across its move, and never
     * shown for another computer saved at its old address.
     */
    @Test
    fun aListFollowsItsPairedComputerAlone() {
        val a = DeckWidgetPlan.computerKey("192.0.2.1", "pc-a")
        assertEquals(a, DeckWidgetPlan.computerKey("192.0.2.9", "pc-a"))
        assertTrue(DeckWidgetPlan.keeps(a, DeckWidgetPlan.computerKey("192.0.2.9", "pc-a")))
        assertFalse(DeckWidgetPlan.keeps(a, DeckWidgetPlan.computerKey("192.0.2.1", "pc-b")))
        assertFalse(DeckWidgetPlan.keeps(a, DeckWidgetPlan.computerKey("192.0.2.1", null)))
        assertEquals("one never paired is its address", "192.0.2.1", DeckWidgetPlan.computerKey("192.0.2.1", null))
    }

    /** AZ2: another computer's list goes, and so does one with no computer left. */
    @Test
    fun aListNoLongerShownIsForgotten() {
        assertTrue(DeckWidgetPlan.forgets(listFor = "a", lastComputer = "b"))
        assertTrue(DeckWidgetPlan.forgets(listFor = "a", lastComputer = null))
        assertFalse(DeckWidgetPlan.forgets(listFor = "a", lastComputer = "a"))
        assertFalse(DeckWidgetPlan.forgets(listFor = null, lastComputer = "a"))
    }
}
