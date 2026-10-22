package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsSearchTest {
    private val ring = listOf("Ring from the computer", "The computer can make this phone ring, loud, for 30 s.")

    @Test
    fun `nothing typed shows every row`() {
        assertTrue(SettingsSearch.rowShown("", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("   ", ring, "Files and links"))
        assertFalse(SettingsSearch.searching(" \t "))
        assertTrue(SettingsSearch.groupShown("", rowsFound = 0))
    }

    @Test
    fun `every word must be found, in any case and order, across title, summary and group`() {
        assertTrue(SettingsSearch.rowShown("RING", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("loud ring", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("ring files", ring, "Files and links"), "the group's name counts")
        assertFalse(SettingsSearch.rowShown("ring bluetooth", ring, "Files and links"))
    }

    @Test
    fun `a group shows while one of its rows is found`() {
        assertFalse(SettingsSearch.groupShown("ring", rowsFound = 0))
        assertTrue(SettingsSearch.groupShown("ring", rowsFound = 2))
    }

    @Test
    fun `the words are split on any space`() {
        assertEquals(listOf("ring", "loud"), SettingsSearch.words("  Ring \n  LOUD "))
    }

    private fun assertTrue(condition: Boolean, message: String) = org.junit.Assert.assertTrue(message, condition)
}
