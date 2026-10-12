package dev.linuxlink.android.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TrackpadGestureTest {
    private fun pad(sensitivity: Float = 1f) = TrackpadGesture(slop = 8f, sensitivity = sensitivity)

    private val left = PadOutput.Click(TrackpadGesture.LEFT)

    @Test
    fun aOneFingerTapIsALeftClick() {
        val pad = pad()
        pad.down(0)
        pad.move(1f, 1f, fingers = 1)
        assertEquals(listOf(left), pad.up(100))
    }

    @Test
    fun twoAndThreeFingerTapsAreRightAndMiddleClicks() {
        val pad = pad()
        pad.down(0)
        pad.move(0f, 0f, fingers = 2)
        assertEquals(listOf(PadOutput.Click(TrackpadGesture.RIGHT)), pad.up(150))
        pad.down(1_000)
        pad.move(0f, 0f, fingers = 3)
        assertEquals(listOf(PadOutput.Click(TrackpadGesture.MIDDLE)), pad.up(1_150))
    }

    @Test
    fun restingTwoFingersIsNotARightClick() {
        val pad = pad()
        pad.down(0)
        pad.move(0f, 0f, fingers = 2)
        assertEquals(emptyList<PadOutput>(), pad.up(1_000))
    }

    @Test
    fun oneFingerMovesTheCursorOnceItPassesTheSlop() {
        val pad = pad()
        pad.down(0)
        assertEquals("inside the slop", emptyList<PadOutput>(), pad.move(3f, 0f, fingers = 1))
        assertEquals(listOf(PadOutput.Move(10, 0)), pad.move(10f, 0f, fingers = 1))
        assertEquals("a move is not a tap", emptyList<PadOutput>(), pad.up(200))
    }

    @Test
    fun sensitivityScalesMotionAndKeepsTheFraction() {
        val pad = pad(sensitivity = 1.5f)
        pad.down(0)
        pad.move(10f, 0f, fingers = 1)
        assertEquals(listOf(PadOutput.Move(1, 0)), pad.move(1f, 0f, fingers = 1)) // 1.5 → 1, carry 0.5
        assertEquals(listOf(PadOutput.Move(2, 0)), pad.move(1f, 0f, fingers = 1)) // 0.5 + 1.5 → 2
    }

    @Test
    fun twoFingersScrollContentWithTheFingers() {
        val pad = pad()
        pad.down(0)
        assertEquals(listOf(PadOutput.Scroll(0, -3)), pad.move(0f, 3f, fingers = 2))
        assertEquals(listOf(PadOutput.Scroll(4, 0)), pad.move(4f, 0f, fingers = 2))
        assertEquals(emptyList<PadOutput>(), pad.up(500))
    }

    @Test
    fun tapThenTouchAndMoveDragsWithTheButtonHeld() {
        val pad = pad()
        pad.down(0)
        assertEquals(listOf(left), pad.up(80))
        pad.down(250)
        val start = pad.move(12f, 0f, fingers = 1)
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, true), PadOutput.Move(12, 0)), start)
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, false)), pad.up(900))
    }

    @Test
    fun aSecondTapInTheWindowIsADoubleClickAndATouchLaterIsNotADrag() {
        val pad = pad()
        pad.down(0)
        pad.up(80)
        pad.down(200)
        assertEquals("double click", listOf(left), pad.up(260))
        pad.down(2_000)
        val moved = pad.move(12f, 0f, fingers = 1)
        assertTrue("no button outside the window", moved.none { it is PadOutput.Button })
    }

    @Test
    fun aDragOverlappingASecondFingerStillReleases() {
        val pad = pad()
        pad.down(0)
        pad.up(80)
        pad.down(200)
        pad.move(12f, 0f, fingers = 1)
        pad.move(5f, 5f, fingers = 2)
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, false)), pad.up(800))
    }
}
