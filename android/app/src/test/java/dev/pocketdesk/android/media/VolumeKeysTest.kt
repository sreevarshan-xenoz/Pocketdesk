package dev.pocketdesk.android.media

import org.junit.Assert.assertEquals
import org.junit.Test

class VolumeKeysTest {
    /** K4: the computer's percent and the panel's steps of 5 %. */
    @Test
    fun `a percent is a step and back`() {
        assertEquals(0, VolumeKeys.toStep(0))
        assertEquals(9, VolumeKeys.toStep(45))
        assertEquals("to the nearest step", 9, VolumeKeys.toStep(47))
        assertEquals(10, VolumeKeys.toStep(48))
        assertEquals(20, VolumeKeys.toStep(100))
        assertEquals(20, VolumeKeys.toStep(150))
        assertEquals(0, VolumeKeys.toStep(-3))
        assertEquals(45, VolumeKeys.toPercent(9))
        assertEquals(100, VolumeKeys.toPercent(25))
    }

    /** K4: a press is one step, never past either end. */
    @Test
    fun `a press moves one step and stops at the ends`() {
        assertEquals(10, VolumeKeys.adjust(9, 1))
        assertEquals(8, VolumeKeys.adjust(9, -1))
        assertEquals(9, VolumeKeys.adjust(9, 0))
        assertEquals(20, VolumeKeys.adjust(20, 1))
        assertEquals(0, VolumeKeys.adjust(0, -1))
        assertEquals("one step whatever it says", 10, VolumeKeys.adjust(9, 5))
    }
}
