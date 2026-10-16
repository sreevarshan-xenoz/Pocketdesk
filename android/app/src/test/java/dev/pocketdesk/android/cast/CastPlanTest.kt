package dev.pocketdesk.android.cast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CastPlanTest {
    @Test
    fun aTallPhoneIsScaledToTheLongSideAndKeepsItsShape() {
        assertEquals(864 to 1920, CastPlan.encodeSize(1080, 2400))
        assertEquals(1920 to 864, CastPlan.encodeSize(2400, 1080), "turned, it turns")
    }

    @Test
    fun aSmallScreenIsNotEnlarged() {
        assertEquals(720 to 1280, CastPlan.encodeSize(720, 1280))
    }

    @Test
    fun bothSidesAreMultiplesOfSixteen() {
        val (w, h) = CastPlan.encodeSize(1080, 2340)
        assertEquals(0, w % 16)
        assertEquals(0, h % 16)
        assertTrue(h <= CastPlan.MAX_LONG_SIDE)
        assertEquals(1080.0 / 2340, w.toDouble() / h, 0.01)
    }

    @Test
    fun aDegenerateScreenStillGetsAPicture() {
        assertEquals(16 to 16, CastPlan.encodeSize(0, 0))
    }

    @Test
    fun onlyAFullQueueAsksForAKeyframe() {
        assertTrue(CastPlan.wantsKeyframe(CastPlan.FULL))
        assertFalse(CastPlan.wantsKeyframe(CastPlan.QUEUED))
        assertFalse(CastPlan.wantsKeyframe(CastPlan.NO_CAST))
    }

    private fun assertEquals(expected: Pair<Int, Int>, actual: Pair<Int, Int>, message: String) =
        org.junit.Assert.assertEquals(message, expected, actual)
}
