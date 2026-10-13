package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayoutPolicyTest {
    @Test
    fun lateAndRepeatedPacketsAreDropped() {
        val policy = PlayoutPolicy()
        assertTrue(policy.accept(0))
        assertTrue(policy.accept(1))
        assertFalse("repeat", policy.accept(1))
        assertFalse("late", policy.accept(0))
        assertTrue("a gap is fine", policy.accept(5))
    }

    @Test
    fun aSequenceFarBehindIsARestart() {
        val policy = PlayoutPolicy()
        assertTrue(policy.accept(1_000))
        assertTrue("a new audio task counts from 0", policy.accept(0))
        assertTrue(policy.accept(1))
    }

    @Test
    fun playbackStartsAtTheStartLevel() {
        val policy = PlayoutPolicy()
        assertFalse(policy.readyToPlay(40))
        assertTrue(policy.readyToPlay(60))
    }

    @Test
    fun pastTheCapFramesAreSkippedUntilBackAtTheStartLevel() {
        val policy = PlayoutPolicy()
        assertTrue(policy.shouldWrite(100))
        assertFalse("over the cap", policy.shouldWrite(151))
        assertFalse("still draining", policy.shouldWrite(100))
        assertTrue("back at the start level", policy.shouldWrite(60))
        assertTrue(policy.shouldWrite(100))
    }

    @Test
    fun aStarvedTrackPausesToRefill() {
        val policy = PlayoutPolicy()
        assertTrue(policy.starved(playing = true, queuedMs = 0))
        assertFalse(policy.starved(playing = true, queuedMs = 20))
        assertFalse("not playing yet", policy.starved(playing = false, queuedMs = 0))
    }

    @Test
    fun rebufferEndsADrain() {
        val policy = PlayoutPolicy()
        policy.shouldWrite(200)
        policy.rebuffer()
        assertTrue(policy.shouldWrite(100))
    }

    @Test
    fun anEmptyPollThatReturnedAtOnceWaitsOutTheRestOfItsSlot() {
        // The bridge returns at once when the session is gone or closed;
        // without this the playout thread spins at full priority.
        assertEquals(20, PlayoutPolicy.idlePauseMs(elapsedMs = 0, empty = true))
        assertEquals(5, PlayoutPolicy.idlePauseMs(elapsedMs = 15, empty = true))
        assertEquals("a poll that waited its timeout", 0, PlayoutPolicy.idlePauseMs(elapsedMs = 20, empty = true))
        assertEquals("packets came", 0, PlayoutPolicy.idlePauseMs(elapsedMs = 0, empty = false))
    }

    @Test
    fun queuedTimeIsRightAcrossTheHeadWrap() {
        assertEquals(60, PlayoutPolicy.queuedMs(framesWritten = 48 * 60L, headPosition = 0))
        // Written count has wrapped past 2^32; the head has not yet.
        val written = (0xFFFF_FFFFL + 1 + 48 * 10) and 0xFFFF_FFFFL
        val head = (0xFFFF_FFFFL - 48 * 50 + 1).toInt()
        assertEquals(60, PlayoutPolicy.queuedMs(written, head))
    }
}
