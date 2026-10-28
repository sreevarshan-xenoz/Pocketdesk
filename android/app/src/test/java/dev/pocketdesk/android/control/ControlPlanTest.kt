package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.ControlPlan.Point
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ControlPlanTest {
    @Test
    fun aFractionLandsOnTheScreenAsItIsDrawn() {
        assertEquals(Point(0f, 0f), ControlPlan.place(0, 0, 1080, 2400))
        assertEquals(Point(1079f, 2399f), ControlPlan.place(65535, 65535, 1080, 2400))
        // Turned: the same fraction, the other shape.
        val middle = ControlPlan.place(32768, 32768, 2400, 1080)
        assertEquals(1199.5f, middle.x, 0.5f)
        assertEquals(539.5f, middle.y, 0.5f)
        assertEquals("never off the screen", Point(1079f, 0f), ControlPlan.place(99999, -5, 1080, 2400))
    }

    @Test
    fun aPressAndReleaseIsOneTap() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(10f, 20f), 0)
        strokes.up(Point(10f, 20f), 5)
        val tap = strokes.next(6)!!
        assertTrue(tap.start)
        assertFalse(tap.willContinue)
        assertEquals(ControlPlan.TAP_MS, tap.durationMs)
        assertFalse("done", strokes.active)
        assertNull(strokes.next(7))
    }

    @Test
    fun aDragFollowsThePointerJoiningWhatArrivedMeanwhile() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(0f, 0f), 0)
        strokes.move(Point(5f, 0f), 16)
        val first = strokes.next(16)!!
        assertTrue(first.start)
        assertTrue("the finger stays down", first.willContinue)
        assertEquals(listOf(Point(0f, 0f), Point(5f, 0f)), first.points)
        // Three moves arrive while the first segment is performed: one segment.
        strokes.move(Point(10f, 0f), 32)
        strokes.move(Point(15f, 0f), 48)
        strokes.move(Point(20f, 0f), 64)
        val second = strokes.next(66)!!
        assertFalse(second.start)
        assertEquals("continues from where the last ended", Point(5f, 0f), second.points.first())
        assertEquals(Point(20f, 0f), second.points.last())
        assertEquals(4, second.points.size)
        assertEquals(50L, second.durationMs)
        strokes.up(Point(20f, 5f), 80)
        val last = strokes.next(82)!!
        assertFalse("the finger lifts", last.willContinue)
        assertEquals(Point(20f, 0f), last.points.first())
        assertFalse(strokes.active)
    }

    @Test
    fun aPressHeldStillStartsAndHoldsALongPress() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(1f, 1f), 0)
        assertNull("too soon: it may be a tap", strokes.next(30))
        assertTrue(strokes.waiting(ControlPlan.START_WAIT_MS))
        val hold = strokes.next(ControlPlan.START_WAIT_MS)!!
        assertTrue(hold.start)
        assertTrue(hold.willContinue)
        assertNull("nothing new while held", strokes.next(500))
        strokes.up(Point(1f, 1f), 900)
        val release = strokes.next(900)!!
        assertFalse(release.willContinue)
        assertEquals("a late segment is not replayed slowly", ControlPlan.MAX_SEGMENT_MS, release.durationMs)
    }

    @Test
    fun aFingerLeftDownIsLifted() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(3f, 3f), 0)
        strokes.move(Point(4f, 3f), 10)
        strokes.next(10)
        strokes.liftIfIdle(ControlPlan.IDLE_LIFT_MS - 1)
        assertNull("not idle yet", strokes.next(ControlPlan.IDLE_LIFT_MS - 1))
        strokes.liftIfIdle(ControlPlan.IDLE_LIFT_MS + 10)
        val lift = strokes.next(ControlPlan.IDLE_LIFT_MS + 10)!!
        assertFalse(lift.willContinue)
        assertFalse(strokes.active)
    }

    /**
     * BK5 review I2: a button held still on the phone as a screen stays
     * down while the computer is still there (its pings keep it); a press
     * nothing keeps is still lifted after C8's two seconds, and keeping
     * nothing down, or a finger already let go, starts nothing.
     */
    @Test
    fun aFingerKeptDownOutlastsTheIdleLift() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(3f, 3f), 0)
        strokes.next(ControlPlan.START_WAIT_MS)
        for (t in 200L..4_000L step 200L) {
            strokes.keep(t)
            strokes.liftIfIdle(t)
            assertNull("held at $t", strokes.next(t))
        }
        assertTrue(strokes.active)
        // The computer gone quiet: the idle lift still comes.
        strokes.liftIfIdle(4_000 + ControlPlan.IDLE_LIFT_MS)
        assertFalse(strokes.next(4_000 + ControlPlan.IDLE_LIFT_MS)!!.willContinue)
        assertFalse(strokes.active)
        // Kept with no finger down, or after the release: nothing.
        strokes.keep(7_000)
        assertNull(strokes.next(7_000))
        strokes.down(Point(1f, 1f), 7_000)
        strokes.up(Point(1f, 1f), 7_010)
        strokes.keep(7_020)
        assertFalse(strokes.next(7_020)!!.willContinue)
        assertFalse(strokes.active)
    }

    @Test
    fun aFingerDownIsLiftedWhereItIs() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(1f, 1f), 0)
        strokes.move(Point(2f, 2f), 10)
        strokes.next(10)
        strokes.move(Point(3f, 3f), 20)
        strokes.lift(30)
        val last = strokes.next(30)!!
        assertFalse(last.willContinue)
        assertEquals(Point(3f, 3f), last.points.last())
        assertFalse(strokes.active)
        strokes.lift(40)
        assertNull("nothing down: nothing to lift", strokes.next(40))
    }

    @Test
    fun movesWaitingForASegmentAreBounded() {
        val strokes = ControlPlan.Strokes()
        strokes.down(Point(0f, 0f), 0)
        repeat(ControlPlan.MAX_PENDING + 50) { strokes.move(Point(it.toFloat(), 0f), it.toLong()) }
        val segment = strokes.next(1_000)!!
        assertEquals(ControlPlan.MAX_PENDING + 1, segment.points.size)
        assertEquals("ends where the pointer is", Point((ControlPlan.MAX_PENDING + 49).toFloat(), 0f), segment.points.last())
    }

    @Test
    fun aMoveOrReleaseWithoutAPressIsIgnored() {
        val strokes = ControlPlan.Strokes()
        strokes.move(Point(1f, 1f), 0)
        strokes.up(Point(1f, 1f), 1)
        assertNull(strokes.next(2))
        assertFalse(strokes.active)
    }

    @Test
    fun textGoesInAtTheSelection() {
        assertEquals(ControlPlan.Edited("hello world", 11), ControlPlan.type("hello", -1, -1, " world"))
        assertEquals(ControlPlan.Edited("heyllo", 3), ControlPlan.type("hello", 2, 2, "y"))
        assertEquals("replaces the selection", ControlPlan.Edited("hXo", 2), ControlPlan.type("hello", 1, 4, "X"))
        assertEquals("a backwards selection", ControlPlan.Edited("hXo", 2), ControlPlan.type("hello", 4, 1, "X"))
    }

    @Test
    fun backspaceDeletesTheSelectionOrOneWholeCharacter() {
        assertEquals(ControlPlan.Edited("hell", 4), ControlPlan.backspace("hello", -1, -1))
        assertEquals(ControlPlan.Edited("ho", 1), ControlPlan.backspace("hello", 1, 4))
        assertEquals(ControlPlan.Edited("hello", 0), ControlPlan.backspace("hello", 0, 0))
        val emoji = "a😀"
        assertEquals("a surrogate pair is one character", ControlPlan.Edited("a", 1), ControlPlan.backspace(emoji, 3, 3))
    }

    @Test
    fun atMostTwoHundredEventsASecondAreTaken() {
        val rate = ControlPlan.Rate()
        repeat(ControlPlan.MAX_EVENTS_PER_SECOND) { assertTrue(rate.take(1_000)) }
        assertFalse(rate.take(1_500))
        assertTrue("a new second", rate.take(2_000))
    }

    @Test
    fun aWheelIsASwipeAgainstTheContent() {
        val down = ControlPlan.scroll(Point(500f, 1200f), 1, 2400)
        assertEquals(Point(500f, 960f), down.last())
        val up = ControlPlan.scroll(Point(500f, 1200f), -2, 2400)
        assertEquals(Point(500f, 1680f), up.last())
        val far = ControlPlan.scroll(Point(500f, 100f), 9, 2400)
        assertEquals("never off the screen", 0f, far.last().y)
    }
}
