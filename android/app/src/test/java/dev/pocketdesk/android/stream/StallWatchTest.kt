package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StallWatchTest {
    private fun watch() = StallWatch(stallMs = 10_000, firstFrameMs = 45_000, startedAt = 0)

    @Test
    fun aLinkWithNeitherFramesNorBytesForTenSecondsIsDead() {
        val w = watch()
        w.frame(at = 1_000)
        assertFalse(w.stalled(at = 10_999, videoBytes = 0))
        assertTrue(w.stalled(at = 11_001, videoBytes = 0))
    }

    @Test
    fun aSlowFrameStillArrivingKeepsTheLinkAlive() {
        val w = watch()
        w.frame(at = 1_000)
        // A large keyframe coming in slowly: bytes, no frame yet.
        assertFalse(w.stalled(at = 6_000, videoBytes = 40_000))
        assertFalse(w.stalled(at = 14_000, videoBytes = 90_000))
        assertFalse(w.stalled(at = 23_000, videoBytes = 150_000))
        // Then nothing at all for ten seconds: dead.
        assertTrue(w.stalled(at = 33_001, videoBytes = 150_000))
    }

    /** The phone's watch (BG6): quiet at 3 s of silence on the connection, dead at 8 s. */
    private fun quickWatch() = StallWatch.forPhone(startedAt = 0)

    @Test
    fun theLimitsAreInOnePlace() {
        assertTrue(StallWatch.QUIET_NOTICE_MS < StallWatch.QUIET_DEAD_MS)
        assertTrue(StallWatch.QUIET_DEAD_MS < StallWatch.STALL_MS)
        assertEquals(3_000L, StallWatch.QUIET_NOTICE_MS)
        assertEquals(8_000L, StallWatch.QUIET_DEAD_MS)
    }

    @Test
    fun aStillScreenOnALiveLinkIsAliveAsLongAsTheBackstopAllows() {
        // The desktop sends no frame and no byte (a still screen with no idle repeats), but
        // the connection's pings are answered, so it hears from it every second: the link is
        // never quiet. (The 10 s byte and frame watch is the backstop, as before.)
        val w = quickWatch()
        w.frame(at = 1_000)
        var at = 1_000L
        while (at < 10_900) {
            at += 50
            assertEquals("at $at", LinkState.Alive, w.check(at, videoBytes = 0, linkQuietMs = at % 1_000))
        }
    }

    @Test
    fun aSlowKeyframeOnALiveLinkIsNeverQuietOrDead() {
        val w = quickWatch()
        w.frame(at = 1_000)
        var bytes = 0L
        var at = 1_000L
        // 30 s of one frame arriving slowly: bytes rising, the connection heard from throughout.
        while (at < 31_000) {
            at += 50
            bytes += 400
            assertEquals("at $at", LinkState.Alive, w.check(at, bytes, linkQuietMs = 100))
        }
    }

    @Test
    fun aLinkThatWentAwayIsQuietAtThreeSecondsAndDeadAtEight() {
        val w = quickWatch()
        val diedAt = 5_000L
        var at = 0L
        var quietAt = -1L
        var deadAt = -1L
        while (at < 30_000 && deadAt < 0) {
            at += 50
            if (at < diedAt) w.frame(at)
            // Wi-Fi off: nothing more is heard, and the quiet grows from the moment it went.
            val quiet = if (at < diedAt) 0L else at - diedAt
            when (w.check(at, videoBytes = if (at < diedAt) at else diedAt, linkQuietMs = quiet)) {
                LinkState.Alive -> Unit
                LinkState.Quiet -> if (quietAt < 0) quietAt = at
                LinkState.Dead -> deadAt = at
            }
        }
        // Said to be reconnecting after about 3 s, ended after about 8 s: the byte and frame
        // watch alone took 10 s to say anything.
        assertTrue("quiet ${quietAt - diedAt} ms after it died", quietAt - diedAt in 3_000..3_100)
        assertTrue("dead ${deadAt - diedAt} ms after it died", deadAt - diedAt in 8_000..8_100)
        val slow = watch()
        slow.frame(at = diedAt)
        assertFalse(slow.stalled(at = diedAt + 8_100, videoBytes = 0))
    }

    @Test
    fun aFiveSecondBlackoutRecoversWithoutTeardown() {
        // A Wi-Fi roam, a burst of loss, a power-save gap: nothing is heard for 5 s, then the
        // link is back. It is quiet in between and never dead, and the video's own backstop is
        // not tripped by the gap either.
        val w = quickWatch()
        val blackoutFrom = 10_000L
        val blackoutTo = 15_000L
        var at = 0L
        var bytes = 0L
        var sawQuiet = false
        while (at < 40_000) {
            at += 50
            val blackout = at in blackoutFrom until blackoutTo
            if (!blackout) {
                bytes += 300
                if (at % 100 == 0L) w.frame(at)
            }
            val quiet = if (blackout) at - blackoutFrom else 0L
            val state = w.check(at, bytes, linkQuietMs = quiet)
            assertTrue("torn down at $at", state != LinkState.Dead)
            if (state == LinkState.Quiet) sawQuiet = true
        }
        assertTrue("it was said to be reconnecting", sawQuiet)
    }

    @Test
    fun theQuietStateClearsWhenTheLinkIsHeardFromAgain() {
        val w = quickWatch()
        w.frame(at = 1_000)
        assertEquals(LinkState.Alive, w.check(at = 2_000, videoBytes = 10, linkQuietMs = 1_000))
        assertEquals(LinkState.Quiet, w.check(at = 4_100, videoBytes = 10, linkQuietMs = 3_100))
        assertEquals(LinkState.Quiet, w.check(at = 6_000, videoBytes = 10, linkQuietMs = 5_000))
        // Datagrams are rising again: the count's quiet is back to zero.
        assertEquals(LinkState.Alive, w.check(at = 6_050, videoBytes = 10, linkQuietMs = 0))
        assertEquals(LinkState.Alive, w.check(at = 7_000, videoBytes = 900, linkQuietMs = 100))
    }

    @Test
    fun eightSecondsOfSilenceEndsTheSession() {
        val w = quickWatch()
        w.frame(at = 1_000)
        assertEquals(LinkState.Quiet, w.check(at = 9_000, videoBytes = 0, linkQuietMs = 8_000))
        assertEquals(LinkState.Dead, w.check(at = 9_050, videoBytes = 0, linkQuietMs = 8_001))
    }

    @Test
    fun aClosedConnectionEndsItAtOnce() {
        val w = quickWatch()
        w.frame(at = 1_000)
        assertEquals(LinkState.Alive, w.check(at = 1_100, videoBytes = 5, linkQuietMs = 0))
        assertEquals(LinkState.Dead, w.check(at = 1_150, videoBytes = 5, linkQuietMs = 0, closed = true))
    }

    @Test
    fun aQuietLinkIsQuietEvenBeforeTheFirstFrame() {
        // The desktop has not answered the session yet, and nothing is heard from it either.
        val w = quickWatch()
        assertEquals(LinkState.Alive, w.check(at = 2_000, videoBytes = 0, linkQuietMs = 3_000))
        assertEquals(LinkState.Quiet, w.check(at = 2_050, videoBytes = 0, linkQuietMs = 3_001))
        assertEquals(LinkState.Dead, w.check(at = 9_000, videoBytes = 0, linkQuietMs = 8_001))
    }

    @Test
    fun aDesktopThatAsksFirstIsWaitedForWhileTheLinkIsHeard() {
        // The 45 s wait for the first frame stands while the connection answers.
        val w = quickWatch()
        assertFalse(w.stalled(at = 44_000, videoBytes = 0, linkQuietMs = 200))
        assertTrue(w.stalled(at = 45_001, videoBytes = 0, linkQuietMs = 200))
    }

    @Test
    fun theByteAndFrameWatchStillCatchesALinkThatIsHeardButSilent() {
        // The backstop: the connection answers pings but no video comes for 10 s.
        val w = quickWatch()
        w.frame(at = 1_000)
        assertFalse(w.stalled(at = 10_999, videoBytes = 0, linkQuietMs = 100))
        assertTrue(w.stalled(at = 11_001, videoBytes = 0, linkQuietMs = 100))
    }

    @Test
    fun theFirstFrameIsWaitedForLonger() {
        // A desktop that asks before each session holds the stream.
        val w = watch()
        assertFalse(w.stalled(at = 44_000, videoBytes = 0))
        assertTrue(w.stalled(at = 45_001, videoBytes = 0))
    }
}
