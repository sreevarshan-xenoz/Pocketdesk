package dev.linuxlink.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenChoicesTest {
    private val screens = listOf(
        DesktopMonitor(index = 0, name = "\\\\.\\DISPLAY3", width = 2560, height = 1080, isPrimary = true),
        DesktopMonitor(index = 1, name = "\\\\.\\DISPLAY1", width = 2560, height = 1440, isPrimary = false),
    )

    @Test
    fun autoMeansThePrimaryScreen() {
        assertEquals(0, ScreenChoices.current(screens, selected = -1))
    }

    @Test
    fun aChosenScreenIsCurrent() {
        assertEquals(1, ScreenChoices.current(screens, selected = 1))
    }

    @Test
    fun aScreenThatWasUnpluggedFallsBackToThePrimary() {
        assertEquals(0, ScreenChoices.current(screens, selected = 2))
    }

    @Test
    fun theSwitcherShowsOnlyWithMoreThanOneScreen() {
        assertTrue(ScreenChoices.worthShowing(screens))
        assertFalse(ScreenChoices.worthShowing(screens.take(1)))
        assertFalse(ScreenChoices.worthShowing(emptyList()))
    }

    @Test
    fun tappingTheScreenAlreadyShownDoesNothing() {
        assertFalse(ScreenChoices.isSwitch(screens, selected = -1, tapped = 0))
        assertTrue(ScreenChoices.isSwitch(screens, selected = -1, tapped = 1))
        assertFalse(ScreenChoices.isSwitch(screens, selected = 1, tapped = 1))
    }
}
