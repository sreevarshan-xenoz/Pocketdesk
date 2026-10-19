package dev.pocketdesk.android.stream

import dev.pocketdesk.android.bridge.RustCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI

class PenStrokeTest {
    private val at = 100 to 200

    @Test
    fun aFingerDrawsInContactAndLiftsOutOfRange() {
        val pen = PenStroke(PenCurve.Even)
        val down = pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 0.5f)!!
        assertEquals(RustCore.PEN_CONTACT or RustCore.PEN_IN_RANGE, down.flags)
        assertEquals(32768, down.pressure)
        assertEquals(100 to 200, down.xNorm to down.yNorm)
        val up = pen.event(at, contact = false, hovering = false, fingers = 0, pressure = 0f)!!
        assertEquals("a finger up leaves range", 0, up.flags)
        assertEquals(0, up.pressure)
        assertNull("nothing more to say", pen.event(at, contact = false, hovering = false, fingers = 0, pressure = 0f))
    }

    @Test
    fun aStylusHoversWithoutPressingAndKeepsItsButton() {
        val pen = PenStroke(PenCurve.Even)
        val hover = pen.event(at, contact = false, hovering = true, fingers = 0, pressure = 0.7f, barrel = true)!!
        assertEquals(RustCore.PEN_IN_RANGE or RustCore.PEN_BARREL, hover.flags)
        assertEquals("a hovering pen presses nothing", 0, hover.pressure)
    }

    @Test
    fun aStylusTouchingRightAfterItsHoverEndsStaysInRange() {
        val pen = PenStroke(PenCurve.Even)
        pen.event(at, contact = false, hovering = true, fingers = 0, pressure = 0f)
        assertNull(
            "the hover's end waits",
            pen.event(at, contact = false, hovering = false, fingers = 0, pressure = 0f, exiting = true),
        )
        val down = pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 0.5f)!!
        assertEquals(RustCore.PEN_CONTACT or RustCore.PEN_IN_RANGE, down.flags)
        assertNull("touched: nothing left to expire", pen.expireExit())
    }

    @Test
    fun aStylusThatLeavesForGoodLeavesOnceTheGraceEnds() {
        val pen = PenStroke(PenCurve.Even)
        pen.event(at, contact = false, hovering = true, fingers = 0, pressure = 0f)
        pen.event(at, contact = false, hovering = false, fingers = 0, pressure = 0f, exiting = true)
        assertEquals(0, pen.expireExit()!!.flags)
        assertNull(pen.expireExit())
    }

    @Test
    fun aSecondFingerLiftsThePenUntilEveryFingerIsUp() {
        val pen = PenStroke(PenCurve.Even)
        pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 1f)
        val lift = pen.event(at, contact = true, hovering = false, fingers = 2, pressure = 1f)!!
        assertEquals(0, lift.flags)
        assertNull(pen.event(at, contact = true, hovering = false, fingers = 2, pressure = 1f))
        assertNull("one finger left of a pinch draws nothing", pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 1f))
        assertNull(pen.event(at, contact = false, hovering = false, fingers = 0, pressure = 0f))
        assertTrue("then a new stroke", pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 1f)!!.inRange)
    }

    @Test
    fun aLiftWithNothingInRangeSaysNothing() {
        val pen = PenStroke(PenCurve.Even)
        assertNull(pen.lift())
        pen.event(at, contact = true, hovering = false, fingers = 1, pressure = 1f)
        assertEquals(0, pen.lift()!!.flags)
        assertNull("once", pen.lift())
    }

    @Test
    fun theCurveGivesSoftMoreInkAndFirmLess() {
        val light = 0.25f
        assertTrue(PenCurve.Soft.apply(light) > PenCurve.Even.apply(light))
        assertTrue(PenCurve.Firm.apply(light) < PenCurve.Even.apply(light))
        for (curve in PenCurve.entries) {
            assertEquals(0, curve.apply(0f))
            assertEquals("a finger's pressure above 1 is full", 65535, curve.apply(1.4f))
            assertEquals(0, curve.apply(Float.NaN))
        }
        assertEquals(PenCurve.Even, PenCurve.fromKey("nonsense"))
        assertEquals(PenCurve.Firm, PenCurve.fromKey("firm"))
    }

    @Test
    fun thePenIsOfferedOnlyWhereTheComputerTakesOne() {
        assertEquals(listOf(InputMode.DirectTouch, InputMode.Trackpad), inputModes(pen = false))
        val withPen = inputModes(pen = true)
        assertEquals(InputMode.Pen, nextMode(InputMode.Trackpad, withPen))
        assertEquals(InputMode.DirectTouch, nextMode(InputMode.Pen, withPen))
        assertEquals(
            "a pen no longer offered goes back to the first",
            InputMode.DirectTouch,
            nextMode(InputMode.Pen, inputModes(pen = false)),
        )
    }

    @Test
    fun tiltLeansAlongEachAxis() {
        assertEquals("upright", 0 to 0, penTilt(0f, 1f))
        val leaning = (PI / 4).toFloat()
        // Leaning toward the top of the screen: all along y.
        assertEquals(0 to 45, penTilt(leaning, 0f))
        // A quarter turn: all along x.
        val (x, y) = penTilt(leaning, (PI / 2).toFloat())
        assertEquals(-45, x)
        assertEquals(0, y)
        assertEquals("flat is at most 90", 0 to 90, penTilt((PI / 2).toFloat(), 0f))
    }
}
