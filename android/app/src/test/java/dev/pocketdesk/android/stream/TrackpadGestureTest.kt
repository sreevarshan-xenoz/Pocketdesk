package dev.pocketdesk.android.stream

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
        assertEquals("a tap waits to learn whether it starts a drag", emptyList<PadOutput>(), pad.up(100))
        assertEquals(100 + 300L, pad.pendingDeadline())
        assertEquals("too early", emptyList<PadOutput>(), pad.expire(300))
        assertEquals(listOf(left), pad.expire(400))
        assertEquals("only once", emptyList<PadOutput>(), pad.expire(500))
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
    fun twoFingersScrollNaturallyAboutOneNotchPerPxPerNotch() {
        // Natural: finger down = content down = a positive (up) wheel; finger
        // right = content right = a negative horizontal wheel.
        val pad = TrackpadGesture(slop = 8f, pxPerNotch = 10f)
        pad.down(0)
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 3f, fingers = 2))
        assertEquals("30 px = 3 notches, nothing lost to the slop", listOf(PadOutput.Scroll(0, 3)), pad.move(0f, 27f, fingers = 2))
        assertEquals("half a notch carried, not enough yet", emptyList<PadOutput>(), pad.move(0f, 5f, fingers = 2))
        assertEquals("the carried half completes a notch", listOf(PadOutput.Scroll(-1, 1)), pad.move(10f, 5f, fingers = 2))
        assertEquals(emptyList<PadOutput>(), pad.up(500))
    }

    @Test
    fun tapThenTouchAndMoveDragsWithTheButtonHeld() {
        val pad = pad()
        pad.down(0)
        assertEquals(emptyList<PadOutput>(), pad.up(80))
        pad.down(250)
        val start = pad.move(12f, 0f, fingers = 1)
        // One press and no click before it: the desktop must not see a double click.
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, true), PadOutput.Move(12, 0)), start)
        assertEquals("the tap is spent by the drag", null, pad.pendingDeadline())
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, false)), pad.up(900))
    }

    @Test
    fun aSecondTapInTheWindowIsADoubleClickAndATouchLaterIsNotADrag() {
        val pad = pad()
        pad.down(0)
        pad.up(80)
        pad.down(200)
        assertEquals("double click", listOf(left, left), pad.up(260))
        assertEquals(null, pad.pendingDeadline())
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

    @Test
    fun aLateTouchFlushesTheTapThatWasWaiting() {
        val pad = pad()
        pad.down(0)
        pad.up(80)
        assertEquals("past the window: the click goes out first", listOf(left), pad.down(1_000))
        assertEquals(null, pad.pendingDeadline())
    }

    @Test
    fun aTwoFingerGestureAfterATapFlushesTheTapFirst() {
        val pad = pad()
        pad.down(0)
        pad.up(80)
        pad.down(200)
        val out = pad.move(0f, 20f, fingers = 2)
        assertEquals(left, out.first())
        assertTrue(out.drop(1).all { it is PadOutput.Scroll })
    }

    @Test
    fun cancelReleasesADragAndDropsAWaitingTap() {
        val drag = pad()
        drag.down(0)
        drag.up(80)
        drag.down(200)
        drag.move(12f, 0f, fingers = 1)
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, false)), drag.cancel())
        assertEquals(emptyList<PadOutput>(), drag.cancel())

        val waiting = pad()
        waiting.down(0)
        waiting.up(80)
        assertEquals("a lost click is better than a stuck button", emptyList<PadOutput>(), waiting.cancel())
        assertEquals(null, waiting.pendingDeadline())
    }

    @Test
    fun aTwoFingerTapWithTouchJitterStillOnlyRightClicks() {
        val pad = pad()
        pad.down(0)
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 1f, fingers = 2))
        assertEquals(emptyList<PadOutput>(), pad.move(1f, 0f, fingers = 2))
        assertEquals(listOf(PadOutput.Click(TrackpadGesture.RIGHT)), pad.up(120))
    }

    @Test
    fun twoFingersSpreadingZoomAndNeverScrollOrClick() {
        val pad = pad()
        pad.down(0)
        // The fingers spread (span 100 -> 130) while their centre barely moves.
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 1f, fingers = 2, span = 100f))
        assertEquals(listOf(PadOutput.Zoom(1.3f)), pad.move(0f, 0f, fingers = 2, span = 130f))
        assertEquals(listOf(PadOutput.Zoom(1.5f)), pad.move(0f, 0f, fingers = 2, span = 195f))
        // A pinch stays a pinch: moving together now pans, it does not scroll.
        assertEquals(listOf(PadOutput.Zoom(1f)), pad.move(0f, 40f, fingers = 2, span = 195f))
        assertEquals("a pinch is not a right click", emptyList<PadOutput>(), pad.up(120))
    }

    @Test
    fun twoFingersMovingTogetherStillScrollAndAScrollNeverZooms() {
        val pad = TrackpadGesture(slop = 8f, pxPerNotch = 10f)
        pad.down(0)
        assertEquals(listOf(PadOutput.Scroll(0, 3)), pad.move(0f, 30f, fingers = 2, span = 100f))
        // Once scrolling, the fingers drifting apart does not start a zoom.
        assertEquals(listOf(PadOutput.Scroll(0, 1)), pad.move(0f, 10f, fingers = 2, span = 140f))
        assertEquals(emptyList<PadOutput>(), pad.up(500))
    }

    @Test
    fun withoutASpanTwoFingersOnlyScroll() {
        // The foldable dock gives no span: it never zooms.
        val pad = TrackpadGesture(slop = 8f, pxPerNotch = 10f)
        pad.down(0)
        assertEquals(listOf(PadOutput.Scroll(0, 3)), pad.move(0f, 30f, fingers = 2))
    }

    @Test
    fun aSecondFingerLandingLateCanStillPinch() {
        // One finger drifts past the slop first; only the motion since the
        // second finger landed decides.
        val pad = pad()
        pad.down(0)
        pad.move(0f, 20f, fingers = 1)
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 0f, fingers = 2, span = 100f))
        assertEquals(listOf(PadOutput.Zoom(1.3f)), pad.move(1f, 0f, fingers = 2, span = 130f))
    }

    @Test
    fun aPinchMovingOneFingerIsStillAPinch() {
        // The view gives the centre's motion: half the moving finger's, while
        // the span grows by all of it.
        val pad = pad()
        pad.down(0)
        pad.move(0f, 0f, fingers = 2, span = 100f)
        pad.move(5f, 0f, fingers = 2, span = 110f)
        assertEquals(listOf(PadOutput.Zoom(1.1f)), pad.move(5f, 0f, fingers = 2, span = 121f))
    }

    @Test
    fun aBrokenPairRebaselinesInsteadOfJumping() {
        val pad = pad()
        pad.down(0)
        pad.move(0f, 0f, fingers = 2, span = 100f)
        assertEquals(listOf(PadOutput.Zoom(1.5f)), pad.move(0f, 0f, fingers = 2, span = 150f))
        // A finger lifts (no span), then lands again far away: no 3x jump.
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 0f, fingers = 1))
        assertEquals(emptyList<PadOutput>(), pad.move(0f, 0f, fingers = 2, span = 450f))
        assertEquals(listOf(PadOutput.Zoom(1.1f)), pad.move(0f, 0f, fingers = 2, span = 495f))
        // And one step never zooms more than twice, or less than half.
        assertEquals(listOf(PadOutput.Zoom(2f)), pad.move(0f, 0f, fingers = 2, span = 1500f))
    }

    @Test
    fun threeFingersNeverZoomAndANewGestureStartsUndecided() {
        val pad = TrackpadGesture(slop = 8f, pxPerNotch = 10f)
        pad.down(0)
        assertEquals(listOf(PadOutput.Scroll(0, 3)), pad.move(0f, 30f, fingers = 3, span = 300f))
        pad.up(500)
        pad.down(1000)
        pad.move(0f, 0f, fingers = 2, span = 100f)
        assertEquals(listOf(PadOutput.Zoom(1.3f)), pad.move(0f, 0f, fingers = 2, span = 130f))
    }

    @Test
    fun aDragKeepsGoingWhenASecondFingerLandsAndStillReleases() {
        val pad = pad()
        pad.down(0)
        pad.up(50)
        pad.down(100)
        assertEquals(PadOutput.Button(TrackpadGesture.LEFT, true), pad.move(0f, 20f, fingers = 1).first())
        assertTrue(pad.move(0f, 0f, fingers = 2, span = 200f).none { it is PadOutput.Zoom })
        assertEquals(listOf(PadOutput.Button(TrackpadGesture.LEFT, false)), pad.up(400))
    }
}
