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
    fun threeChoicesAreCrowdedBeforeTheyBecomeAList() {
        assertEquals(ChoiceFit.Layout.Segments, ChoiceFit.layout(3, segmentsFit = true, crowdedFit = true))
        assertEquals(
            "Direct touch, Trackpad and Pen at font scale 1.0 (E6)",
            ChoiceFit.Layout.Crowded,
            ChoiceFit.layout(3, segmentsFit = false, crowdedFit = true),
        )
        assertEquals(ChoiceFit.Layout.List, ChoiceFit.layout(3, segmentsFit = false, crowdedFit = false))
        assertEquals("four are never full segments", ChoiceFit.Layout.Crowded, ChoiceFit.layout(4, segmentsFit = true, crowdedFit = true))
    }

    @Test
    fun theRowFitsOnlyWhenEveryWordFitsItsSegment() {
        assertTrue(ChoiceFit.fits(200f, listOf(120f, 80f, 199.5f)))
        assertTrue(ChoiceFit.fits(200f, listOf(200f)))
        assertFalse("one word too wide: a list", ChoiceFit.fits(200f, listOf(120f, 230f)))
        assertFalse("no room at all", ChoiceFit.fits(0f, listOf(10f)))
    }
}
