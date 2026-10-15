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
}
