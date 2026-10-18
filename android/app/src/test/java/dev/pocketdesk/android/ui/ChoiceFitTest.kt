package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ChoiceFitTest {
    @Test
    fun anOptionIsMeasuredWordByWord() {
        assertEquals(listOf("Balanced"), ChoiceFit.words("Balanced"))
        assertEquals(listOf("Pantalla", "dividida"), ChoiceFit.words("Pantalla dividida"))
        assertEquals(listOf("Max"), ChoiceFit.words(" Max "))
        assertEquals("no break at a no-break space", listOf("Re\u00A0Pág"), ChoiceFit.words("Re\u00A0Pág"))
    }

    @Test
    fun theRowFitsOnlyWhenEveryWordFitsItsSegment() {
        assertTrue(ChoiceFit.fits(200f, listOf(120f, 80f, 199.5f)))
        assertTrue(ChoiceFit.fits(200f, listOf(200f)))
        assertFalse("one word too wide: a list", ChoiceFit.fits(200f, listOf(120f, 230f)))
        assertFalse("no room at all", ChoiceFit.fits(0f, listOf(10f)))
    }
}
