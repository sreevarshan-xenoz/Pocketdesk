package dev.pocketdesk.android.stream

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

    /** The phone's watch (BG6): the connection may be quiet for 3 s. */
    private fun quickWatch() = StallWatch(stallMs = 10_000, firstFrameMs = 45_000, startedAt = 0, quietMs = 3_000)

    @Test
    fun aStillScreenOnALiveLinkIsNotReadAsAQuietLink() {
        // The desktop sends no frame and no byte (a still screen with no idle repeats), but
        // the connection's pings are answered, so it hears from it every second: the quick
        // watch never fires. (The 10 s byte and frame watch is the backstop, as before.)
        val w = quickWatch()
        w.frame(at = 1_000)
        var at = 1_000L
        while (at < 10_900) {
            at += 50
            assertFalse("dead at $at", w.stalled(at, videoBytes = 0, linkQuietMs = at % 1_000))
        }
    }

    @Test
    fun aSlowKeyframeOnALiveLinkIsNeverDead() {
        val w = quickWatch()
        w.frame(at = 1_000)
        var bytes = 0L
        var at = 1_000L
        // 30 s of one frame arriving slowly: bytes rising, the connection heard from throughout.
        while (at < 31_000) {
            at += 50
            bytes += 400
            assertFalse("dead at $at", w.stalled(at, bytes, linkQuietMs = 100))
        }
    }

    @Test
    fun aLinkThatWentAwayIsNoticedInAboutThreeSeconds() {
        val w = quickWatch()
        val diedAt = 5_000L
        var at = 0L
        var noticedAt = -1L
        while (at < 30_000 && noticedAt < 0) {
            at += 50
            if (at < diedAt) w.frame(at)
            // Wi-Fi off: nothing more is heard, and the quiet grows from the moment it went.
            val quiet = if (at < diedAt) 0L else at - diedAt
            if (w.stalled(at, videoBytes = if (at < diedAt) at else diedAt, linkQuietMs = quiet)) noticedAt = at
        }
        assertTrue("noticed", noticedAt > 0)
        // About 3 s, where the byte and frame watch alone took 10 s.
        assertTrue("noticed ${noticedAt - diedAt} ms after it died", noticedAt - diedAt in 3_000..3_100)
        val slow = watch()
        slow.frame(at = diedAt)
        assertFalse(slow.stalled(at = diedAt + 3_100, videoBytes = 0))
    }

    @Test
    fun aQuietLinkIsDeadEvenBeforeTheFirstFrame() {
        // The desktop has not answered the session yet, and nothing is heard from it either.
        val w = quickWatch()
        assertFalse(w.stalled(at = 2_000, videoBytes = 0, linkQuietMs = 3_000))
        assertTrue(w.stalled(at = 2_050, videoBytes = 0, linkQuietMs = 3_001))
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
