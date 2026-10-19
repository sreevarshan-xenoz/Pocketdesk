package dev.pocketdesk.android.stream

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GamepadStateTest {
    @Test
    fun aButtonPressesAndReleasesItsBitOnce() {
        val pad = GamepadState()
        assertTrue(pad.press(PadButton.A, true))
        assertFalse("held already: nothing new", pad.press(PadButton.A, true))
        assertTrue(pad.press(PadButton.X, true))
        assertEquals(PadButton.A or PadButton.X, pad.buttons)
        assertTrue(pad.press(PadButton.A, false))
        assertEquals(PadButton.X, pad.buttons)
    }

    @Test
    fun aStickIsCentredInItsDeadZoneAndHeldToItsRim() {
        assertEquals(0 to 0, GamepadState.stickAxes(0.1f, -0.05f))
        val (x, y) = GamepadState.stickAxes(1f, 0f)
        assertEquals(32767, x)
        assertEquals(0, y)
        val (dx, dy) = GamepadState.stickAxes(1f, 1f)
        assertTrue("a diagonal at the rim is held to it", dx < 32767 && dx > 23000 && dx == dy)
        assertEquals(0 to 0, GamepadState.stickAxes(Float.NaN, 0f))
    }

    @Test
    fun aStickOffCentreKeepsSendingUntilItIsBack() {
        val pad = GamepadState()
        assertFalse(pad.moving)
        assertTrue(pad.stick(left = false, x = 0f, y = -1f))
        assertTrue(pad.moving)
        assertEquals(-32767, pad.axes[3])
        assertFalse("unchanged", pad.stick(left = false, x = 0f, y = -1f))
        assertTrue(pad.stick(left = false, x = 0f, y = 0f))
        assertFalse(pad.moving)
    }

    @Test
    fun aHatIsTheDpadAndLeavesTheOtherButtonsAlone() {
        val pad = GamepadState()
        pad.press(PadButton.B, true)
        assertTrue(pad.hat(-1f, -1f))
        assertEquals(PadButton.B or PadButton.LEFT or PadButton.UP, pad.buttons)
        assertTrue(pad.hat(0f, 0f))
        assertEquals(PadButton.B, pad.buttons)
    }

    @Test
    fun aControllerSKeysAreTheirBitsAndOtherKeysAreNot() {
        assertEquals(PadButton.A, PadButton.ofKey(KeyEvent.KEYCODE_BUTTON_A))
        assertEquals(PadButton.RB, PadButton.ofKey(KeyEvent.KEYCODE_BUTTON_R1))
        assertEquals(PadButton.START, PadButton.ofKey(KeyEvent.KEYCODE_BUTTON_START))
        assertEquals(PadButton.DOWN, PadButton.ofKey(KeyEvent.KEYCODE_DPAD_DOWN))
        assertEquals(0, PadButton.ofKey(KeyEvent.KEYCODE_A))
    }

    @Test
    fun resetLetsEverythingGoAndSaysWhetherAnythingWasHeld() {
        val pad = GamepadState()
        assertFalse(pad.reset())
        pad.press(PadButton.START, true)
        pad.stick(left = true, x = 1f, y = 0f)
        assertTrue(pad.reset())
        assertEquals(0, pad.buttons)
        assertFalse(pad.moving)
    }
}
