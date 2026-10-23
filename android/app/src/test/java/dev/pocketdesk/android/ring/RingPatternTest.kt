package dev.pocketdesk.android.ring

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RingPatternTest {
    @Test
    fun theTorchBlinksOnAndOffEachStep() {
        assertTrue(RingPattern.torchOn(0))
        assertTrue(RingPattern.torchOn(RingPattern.TORCH_STEP_MS - 1))
        assertFalse(RingPattern.torchOn(RingPattern.TORCH_STEP_MS))
        assertTrue(RingPattern.torchOn(2 * RingPattern.TORCH_STEP_MS))
        assertFalse("a clock gone back is off", RingPattern.torchOn(-5))
    }

    @Test
    fun theNextChangeIsTheStepsEnd() {
        assertEquals(RingPattern.TORCH_STEP_MS, RingPattern.nextTorchChangeMs(0))
        assertEquals(100L, RingPattern.nextTorchChangeMs(RingPattern.TORCH_STEP_MS - 100))
    }

    @Test
    fun theVibrationStartsAtOnceAndBuzzes() {
        assertEquals(0L, RingPattern.VIBRATION[0])
        assertTrue(RingPattern.VIBRATION.drop(1).all { it > 0 })
    }
}
