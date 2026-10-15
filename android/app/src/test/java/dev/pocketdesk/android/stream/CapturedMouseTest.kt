package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Test

class CapturedMouseTest {
    // Android's MotionEvent button bits.
    private val primary = 1
    private val secondary = 2
    private val tertiary = 4
    private val back = 8

    @Test
    fun motionMovesTheCursorAndKeepsTheFraction() {
        val mouse = CapturedMouse(sensitivity = 1.5f)
        assertEquals(listOf(PadOutput.Move(1, 3)), mouse.event(1f, 2f, buttons = 0))
        // 0.5 carried from each axis.
        assertEquals(listOf(PadOutput.Move(2, 0)), mouse.event(1f, 0f, buttons = 0))
        assertEquals(emptyList<PadOutput>(), mouse.event(0f, 0f, buttons = 0))
    }

    @Test
    fun buttonsArePressedAndReleasedAsTheyChange() {
        val mouse = CapturedMouse()
        assertEquals(
            listOf(PadOutput.Button(TrackpadGesture.LEFT, true)),
            mouse.event(0f, 0f, buttons = primary),
        )
        assertEquals("held, not pressed again", emptyList<PadOutput>(), mouse.event(0f, 0f, buttons = primary))
        assertEquals(
            listOf(PadOutput.Button(TrackpadGesture.LEFT, false), PadOutput.Button(TrackpadGesture.RIGHT, true)),
            mouse.event(0f, 0f, buttons = secondary),
        )
        assertEquals(
            listOf(PadOutput.Button(TrackpadGesture.RIGHT, false), PadOutput.Button(TrackpadGesture.MIDDLE, true), PadOutput.Button(3, true)),
            mouse.event(0f, 0f, buttons = tertiary or back),
        )
    }

    @Test
    fun theWheelScrollsByNotchesAndReleaseLetsGoOfEverything() {
        val mouse = CapturedMouse()
        // Android: positive vertical scroll is away from the user (up).
        assertEquals(listOf(PadOutput.Scroll(0, 1)), mouse.event(0f, 0f, buttons = 0, vscroll = 1f))
        assertEquals(emptyList<PadOutput>(), mouse.event(0f, 0f, buttons = 0, vscroll = 0.5f))
        assertEquals(listOf(PadOutput.Scroll(0, -1)), mouse.event(0f, 0f, buttons = 0, vscroll = -1.5f))
        mouse.event(0f, 0f, buttons = primary or secondary)
        // Capture lost (focus, the view gone): nothing stays held on the desktop.
        assertEquals(
            listOf(PadOutput.Button(TrackpadGesture.LEFT, false), PadOutput.Button(TrackpadGesture.RIGHT, false)),
            mouse.release(),
        )
        assertEquals(emptyList<PadOutput>(), mouse.release())
    }

    @Test
    fun aBatchedEventsSamplesAllCount() {
        // Android batches a fast mouse's samples: each one is a delta.
        assertEquals(10f, CapturedMouse.batched(listOf(2f, 3f, 1f), 4f), 0f)
        assertEquals(-1f, CapturedMouse.batched(emptyList(), -1f), 0f)
    }

    @Test
    fun theMousesBackAndForwardButtonsAreNotThePhonesBackWhileCaptured() {
        val back = 4 // KeyEvent.KEYCODE_BACK
        val forward = 125 // KeyEvent.KEYCODE_FORWARD
        assertEquals(true, CapturedMouse.swallowsKey(back, fromMouse = true, captured = true))
        assertEquals(true, CapturedMouse.swallowsKey(forward, fromMouse = true, captured = true))
        assertEquals("not captured: the phone's own", false, CapturedMouse.swallowsKey(back, fromMouse = true, captured = false))
        assertEquals("the phone's Back", false, CapturedMouse.swallowsKey(back, fromMouse = false, captured = true))
        assertEquals(false, CapturedMouse.swallowsKey(66, fromMouse = true, captured = true))
    }

    @Test
    fun aForwardButtonAndTheSidewaysWheelGoOut() {
        val mouse = CapturedMouse()
        assertEquals(listOf(PadOutput.Button(4, true)), mouse.event(0f, 0f, buttons = 16))
        assertEquals(listOf(PadOutput.Scroll(1, 0)), mouse.event(0f, 0f, buttons = 16, hscroll = 1f))
        assertEquals(listOf(PadOutput.Move(-2, 0)), mouse.event(-2.5f, 0f, buttons = 16))
    }
}
