package dev.pocketdesk.android.ui

import dev.pocketdesk.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DeckTest {
    /** I2: the sheet and the widget draw a kind with one icon. */
    @Test
    fun eachKindHasItsIconAndAnUnknownOneIsKeys() {
        assertEquals(R.drawable.ic_ll_keyboard, Deck.iconRes("keys"))
        assertEquals(R.drawable.ic_ll_settings, Deck.iconRes("task"))
        assertEquals(R.drawable.ic_ll_play, Deck.iconRes("media"))
        assertEquals(R.drawable.ic_ll_language, Deck.iconRes("link"))
        assertEquals(R.drawable.ic_ll_clipboard, Deck.iconRes("text"))
        assertEquals(R.drawable.ic_ll_keyboard, Deck.iconRes("something new"))
    }

    @Test
    fun aListedEntryIsAButtonOnlyWithAnIndexAndALabel() {
        assertEquals(Deck.Button(0, "Mute mic", "keys"), Deck.button(0, "Mute mic", "keys"))
        assertEquals(Deck.Button(2, "Build", "task"), Deck.button(2, "Build", "task"))
        assertNull(Deck.button(-1, "Lost", "keys"))
        assertNull(Deck.button(4, " ", "text"))
        // A kind from a newer computer is shown as keys.
        assertEquals(Deck.Button(5, "Future", "keys"), Deck.button(5, "Future", "teleport"))
    }

    @Test
    fun aPressSaysWhetherItWentAndWhyNot() {
        assertEquals(Deck.Outcome(true, null), Deck.outcome(true, "ignored"))
        val refused = Deck.outcome(false, "Audio is turned off for this phone on the computer")
        assertEquals(false, refused.ok)
        assertEquals("Audio is turned off for this phone on the computer", refused.reason)
        assertNull(Deck.outcome(false, "").reason)
        assertNull(Deck.outcome(false, null).reason)
        // A task that asks first is done here but waits there.
        assertEquals(Deck.Outcome(true, null, waiting = true), Deck.outcome(true, null, waiting = true))
        assertEquals(false, Deck.outcome(false, "no", waiting = true).waiting)
    }

    @Test
    fun theGridHoldsTwoToFourARow() {
        assertEquals(2, Deck.columns(1))
        assertEquals(3, Deck.columns(3))
        assertEquals(4, Deck.columns(9))
        val buttons = (0 until 7).map { Deck.Button(it, "B$it", "keys") }
        assertEquals(listOf(3, 3, 1), Deck.rows(buttons, 3).map { it.size })
        assertEquals(listOf(4, 3), Deck.rows(buttons, 4).map { it.size })
    }
}
