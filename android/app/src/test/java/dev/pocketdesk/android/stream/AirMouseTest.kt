package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Test

class AirMouseTest {
    @Test
    fun turningLeftAndTiltingUpMoveThePointerLeftAndUp() {
        val air = AirMouse(1f)
        // Turning left is a positive turn about z; tilting up, about x.
        val (dx, dy) = air.step(rateX = 0.6f, rateZ = 1f, dtSec = 0.01f)
        assertEquals(-9, dx)
        assertEquals(-5, dy)
    }

    @Test
    fun aStillHandMovesNothingAndFractionsCarry() {
        val air = AirMouse(1f)
        assertEquals(0 to 0, air.step(rateX = 0.02f, rateZ = -0.02f, dtSec = 0.01f))
        // 0.1 rad/s for 10 ms is 0.9 px: nothing yet, then 1 px once it adds up.
        assertEquals(0 to 0, air.step(rateX = 0f, rateZ = -0.1f, dtSec = 0.004f))
        val moves = (1..3).map { air.step(rateX = 0f, rateZ = -0.1f, dtSec = 0.004f).first }
        assertEquals(1, moves.sum())
    }

    @Test
    fun aPressHoldsItsFirstMovesBackAndATapClicks() {
        val press = AirPress(downMs = 1_000)
        // The thumb's tilt in the first moments moves nothing.
        assertEquals(0 to 0, press.move(3, -2, nowMs = 1_050))
        // Past the hold, the held moves go with the new one.
        assertEquals(7 to -2, press.move(4, 0, nowMs = 1_150))
        assertEquals(false, press.release(nowMs = 1_300))
        val tap = AirPress(downMs = 0)
        tap.move(10, 10, nowMs = 40)
        assertEquals(true, tap.release(nowMs = 200))
        assertEquals(false, AirPress(downMs = 0).release(nowMs = 400))
    }

    @Test
    fun speedScalesAndAPauseIsNotATurn() {
        assertEquals(-18 to 0, AirMouse(2f).step(rateX = 0f, rateZ = 1f, dtSec = 0.01f))
        assertEquals(0 to 0, AirMouse(1f).step(rateX = 5f, rateZ = 5f, dtSec = 0.5f))
        assertEquals(0 to 0, AirMouse(1f).step(rateX = 5f, rateZ = 5f, dtSec = 0f))
    }
}
