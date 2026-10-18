package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class QuietTest {
    @Test
    fun `a notification Do Not Disturb hides is held back, one it lets through is not`() {
        // In Do Not Disturb, Android hides it: held back.
        assertTrue(QuietRule.holdBack(on = true, filter = QuietPlan.PRIORITY, matches = false))
        assertTrue(QuietRule.holdBack(on = true, filter = QuietPlan.NONE, matches = false))
        assertTrue(QuietRule.holdBack(on = true, filter = QuietPlan.ALARMS, matches = false))
        // A priority contact, an allowed app: sent.
        assertFalse(QuietRule.holdBack(on = true, filter = QuietPlan.PRIORITY, matches = true))
        // Not in Do Not Disturb, whatever the ranking says.
        assertFalse(QuietRule.holdBack(on = true, filter = QuietPlan.ALL, matches = false))
        // A filter not known yet (the listener just connecting) holds nothing back.
        assertFalse(QuietRule.holdBack(on = true, filter = QuietPlan.UNKNOWN, matches = false))
        // The setting off.
        assertFalse(QuietRule.holdBack(on = false, filter = QuietPlan.NONE, matches = false))
    }

    @Test
    fun `minutes from the computer`() {
        assertEquals(0, QuietPlan.minutes(0))
        assertEquals(1, QuietPlan.minutes(1))
        assertEquals(60, QuietPlan.minutes(60))
        assertEquals(1440, QuietPlan.minutes(1440))
        assertNull(QuietPlan.minutes(-1))
        assertNull(QuietPlan.minutes(1441))
    }

    @Test
    fun `a mute ends at its time, only while the filter is still the one it set`() {
        val mute = QuietPlan.Mute(untilMs = 10_000, filter = QuietPlan.PRIORITY)
        assertEquals(QuietPlan.Next.WAIT, QuietPlan.next(mute, nowMs = 9_999, filter = QuietPlan.PRIORITY))
        assertEquals(QuietPlan.Next.END, QuietPlan.next(mute, nowMs = 10_000, filter = QuietPlan.PRIORITY))
        // The owner turned Do Not Disturb off, or to another kind, meanwhile: theirs stands.
        assertEquals(QuietPlan.Next.FORGET, QuietPlan.next(mute, nowMs = 5_000, filter = QuietPlan.ALL))
        assertEquals(QuietPlan.Next.FORGET, QuietPlan.next(mute, nowMs = 10_000, filter = QuietPlan.NONE))
        // Not known yet: wait.
        assertEquals(QuietPlan.Next.WAIT, QuietPlan.next(mute, nowMs = 10_000, filter = QuietPlan.UNKNOWN))
        assertEquals(QuietPlan.Next.WAIT, QuietPlan.next(null, nowMs = 10_000, filter = QuietPlan.PRIORITY))
    }

    @Test
    fun `what the phone reports`() {
        val mute = QuietPlan.Mute(untilMs = 3_600_000, filter = QuietPlan.PRIORITY)
        assertEquals(QuietPlan.State(quiet = false, untilUnix = null, byComputer = false), QuietPlan.state(QuietPlan.ALL, null))
        assertEquals(QuietPlan.State(quiet = true, untilUnix = null, byComputer = false), QuietPlan.state(QuietPlan.NONE, null))
        assertEquals(QuietPlan.State(quiet = true, untilUnix = 3_600, byComputer = true), QuietPlan.state(QuietPlan.PRIORITY, mute))
        // The owner's own Do Not Disturb of another kind, with a stale mute not yet forgotten.
        assertEquals(QuietPlan.State(quiet = true, untilUnix = null, byComputer = false), QuietPlan.state(QuietPlan.ALARMS, mute))
        assertEquals(QuietPlan.State(quiet = false, untilUnix = null, byComputer = false), QuietPlan.state(QuietPlan.UNKNOWN, null))
    }
}
