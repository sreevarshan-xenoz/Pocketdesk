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

    /** K4 review: presses sent together move as many steps, from a reading only while fresh. */
    @Test
    fun `presses go together from a fresh reading`() {
        assertEquals(14, VolumeKeys.target(9, 5))
        assertEquals(0, VolumeKeys.target(2, -5))
        assertEquals(20, VolumeKeys.target(18, 4))
        assertEquals(true, VolumeKeys.fresh(1_000, 3_999))
        assertEquals(false, VolumeKeys.fresh(1_000, 4_000))
        assertEquals("never read", false, VolumeKeys.fresh(0, 10))
        assertEquals("a clock that went back", false, VolumeKeys.fresh(5_000, 4_000))
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
