package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsSearchTest {
    private val ring = listOf("Ring from the computer", "The computer can make this phone ring, loud, for 30 s.")
    private val screen = listOf("Keep screen on", "During a streaming session")

    @Test
    fun `nothing typed shows every row`() {
        assertTrue(SettingsSearch.rowShown("", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("   ", ring, "Files and links"))
        assertFalse(SettingsSearch.searching(" \t "))
        assertTrue(SettingsSearch.groupShown("", rowsFound = 0))
    }

    @Test
    fun `every word must start a word, in any case and order, across title, summary and group`() {
        assertTrue(SettingsSearch.rowShown("RING", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("loud ri", ring, "Files and links"))
        assertTrue(SettingsSearch.rowShown("ring files", ring, "Files and links"), "the group's name counts")
        assertFalse(SettingsSearch.rowShown("ring bluetooth", ring, "Files and links"))
        // O8 review: found on the phone, "ring" had found "during" and "streaming".
        assertFalse(SettingsSearch.rowShown("ring", screen, "Session"))
    }

    @Test
    fun `accents are not needed`() {
        assertTrue(SettingsSearch.rowShown("conexion", listOf("Detalles de la conexión"), "Sesión"))
        assertTrue(SettingsSearch.rowShown("sesion", listOf("Mantener la pantalla"), "Sesión"))
    }

    @Test
    fun `a group shows while one of its rows is found`() {
        assertFalse(SettingsSearch.groupShown("ring", rowsFound = 0))
        assertTrue(SettingsSearch.groupShown("ring", rowsFound = 2))
    }

    @Test
    fun `the words are split on any space or mark`() {
        assertEquals(listOf("ring", "loud"), SettingsSearch.words("  Ring, \n  LOUD! "))
    }

    private fun assertTrue(condition: Boolean, message: String) = org.junit.Assert.assertTrue(message, condition)
}
