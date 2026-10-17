package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ComputerNameTest {
    @Test
    fun aNameIsOneShortPrintableLine() {
        assertEquals("Study PC", ComputerName.clean("  Study PC "))
        assertEquals("Ann's desk WARN", ComputerName.clean("Ann's\tdesk\n\nWARN"))
        assertEquals("Desktop", ComputerName.clean("Desk\u202Etop\u200B"))
        assertEquals("[31mred", ComputerName.clean("\u001b[31mred"))
        assertEquals("தமிழ் கணினி", ComputerName.clean("தமிழ் கணினி"))
        assertEquals("x".repeat(40), ComputerName.clean("x".repeat(100)))
        assertEquals("😀".repeat(40), ComputerName.clean("😀".repeat(50)))
        assertNull(ComputerName.clean(" \u200B "))
        assertNull(ComputerName.clean(null))
    }

    @Test
    fun theOwnersNameWinsThenItsOwnThenTheAddress() {
        assertEquals("Work", ComputerName.shown("Work", "DESKTOP-4F2K9QH", "192.0.2.40"))
        assertEquals("DESKTOP-4F2K9QH", ComputerName.shown(null, "DESKTOP-4F2K9QH", "192.0.2.40"))
        assertEquals("192.0.2.40", ComputerName.shown(null, null, "192.0.2.40"))
    }
}
