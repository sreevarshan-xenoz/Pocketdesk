package dev.pocketdesk.android.cast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CastPauseRuleTest {
    private fun pauses(
        screen: Boolean = true,
        shown: Boolean = true,
        appsOn: Boolean = true,
        own: Boolean = true,
        closed: String? = null,
        stoppedByOwner: Boolean = false,
    ) = CastPauseRule.pauses(screen, shown, appsOn, own, closed, stoppedByOwner)

    @Test
    fun `a screen the computer left, or a link that dropped, is paused`() {
        assertTrue(pauses(closed = null))
        assertTrue(pauses(closed = "session ended"))
    }

    @Test
    fun `every other end is an end`() {
        for (closed in listOf(
            "phone screen not allowed",
            "phone screen not shown here",
            "not paired",
            "permissions changed",
            "another phone is the camera now",
            "phone camera failed",
        )) {
            assertFalse(closed, pauses(closed = closed))
        }
    }

    @Test
    fun `only a screen that was shown, to the phone's own computer, with the switch on`() {
        assertFalse(pauses(screen = false))
        assertFalse(pauses(shown = false))
        assertFalse(pauses(appsOn = false))
        assertFalse(pauses(own = false))
        // The owner's Stop, or Android ending the projection, is never paused.
        assertFalse(pauses(stoppedByOwner = true))
    }

    @Test
    fun `a pause lasts ten minutes`() {
        assertEquals(10 * 60_000L, CastPauseRule.PAUSE_MS)
        assertFalse(CastPauseRule.expired(1_000, 1_000 + CastPauseRule.PAUSE_MS - 1))
        assertTrue(CastPauseRule.expired(1_000, 1_000 + CastPauseRule.PAUSE_MS))
    }

    @Test
    fun `it resumes only for the computer it was paused for`() {
        val at = 5_000L
        val soon = at + 60_000L
        fun resume(
            pausedFor: String? = "192.0.2.10",
            asking: String = "192.0.2.10",
            now: Long = soon,
            appsOn: Boolean = true,
            own: Boolean = true,
        ) = CastPauseRule.mayResume(pausedFor, asking, at, now, appsOn, own)
        assertTrue(resume())
        // Another computer never resumes it.
        assertFalse(resume(asking = "192.0.2.11"))
        // Nothing paused.
        assertFalse(resume(pausedFor = null))
        // Run out.
        assertFalse(resume(now = at + CastPauseRule.PAUSE_MS))
        // The switch went off, or the computer is no longer the phone's own.
        assertFalse(resume(appsOn = false))
        assertFalse(resume(own = false))
    }

    @Test
    fun `the notification counts the minutes left, rounding up`() {
        assertEquals(10, CastPauseRule.minutesLeft(0, 0))
        assertEquals(10, CastPauseRule.minutesLeft(0, 1))
        assertEquals(9, CastPauseRule.minutesLeft(0, 60_000))
        assertEquals(1, CastPauseRule.minutesLeft(0, CastPauseRule.PAUSE_MS - 1))
        assertEquals(0, CastPauseRule.minutesLeft(0, CastPauseRule.PAUSE_MS + 5))
    }
}
