package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HomeCardsTest {
    @Test
    fun noneWithNoSavedComputer() {
        assertNull(HomeCards.shown(hasSaved = false, health = true, background = true))
    }

    @Test
    fun healthGoesFirst() {
        assertEquals(HomeCards.Card.HEALTH, HomeCards.shown(hasSaved = true, health = true, background = true))
    }

    @Test
    fun backgroundWhenHealthIsQuiet() {
        assertEquals(HomeCards.Card.BACKGROUND, HomeCards.shown(hasSaved = true, health = false, background = true))
    }

    @Test
    fun noneWhenNothingToSay() {
        assertNull(HomeCards.shown(hasSaved = true, health = false, background = false))
    }
}
