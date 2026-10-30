package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.CardMenu.Action
import dev.pocketdesk.android.ui.CardMenu.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CardMenuTest {
    @Test
    fun `every action is reachable once with everything offered`() {
        val all = CardMenu.sections(peek = true, phoneSound = true, hasWake = true).flatMap { it.actions }
        assertEquals(Action.entries.toSet(), all.toSet())
        assertEquals(all.size, all.toSet().size)
    }

    @Test
    fun `three headed sections, only manage folded`() {
        val sections = CardMenu.sections(peek = true, phoneSound = true, hasWake = true)
        assertEquals(listOf(Kind.COMPUTER, Kind.PHONE, Kind.MANAGE), sections.map { it.kind })
        assertEquals(listOf(false, false, true), sections.map { it.folded })
    }

    @Test
    fun `the open sections stay short`() {
        val open = CardMenu.sections(peek = true, phoneSound = true, hasWake = true).filterNot { it.folded }
        assertEquals(8, open.sumOf { it.actions.size })
    }

    @Test
    fun `remove is the last item`() {
        val manage = CardMenu.sections(peek = false, phoneSound = false, hasWake = false).last()
        assertEquals(Action.REMOVE, manage.actions.last())
    }

    @Test
    fun `offers left out are not shown`() {
        val all = CardMenu.sections(peek = false, phoneSound = false, hasWake = false).flatMap { it.actions }
        assertFalse(Action.PEEK in all)
        assertFalse(Action.PHONE_SOUND in all)
        assertFalse(Action.WAKE in all)
        // Wake settings is upkeep: there before a wake address is set.
        assertTrue(Action.WAKE_SETTINGS in all)
    }

    @Test
    fun `wake leads when offered`() {
        val computer = CardMenu.sections(peek = true, phoneSound = true, hasWake = true).first()
        assertEquals(Action.WAKE, computer.actions.first())
    }
}
