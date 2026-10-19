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
}
