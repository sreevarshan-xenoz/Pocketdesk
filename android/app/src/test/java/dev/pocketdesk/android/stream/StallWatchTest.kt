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

    @Test
    fun theFirstFrameIsWaitedForLonger() {
        // A desktop that asks before each session holds the stream.
        val w = watch()
        assertFalse(w.stalled(at = 44_000, videoBytes = 0))
        assertTrue(w.stalled(at = 45_001, videoBytes = 0))
    }
}
