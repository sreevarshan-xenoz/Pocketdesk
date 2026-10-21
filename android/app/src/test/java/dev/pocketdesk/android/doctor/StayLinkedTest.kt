package dev.pocketdesk.android.doctor

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StayLinkedTest {
    @Test
    fun `runs only with the switch a feature access and a paired computer`() {
        assertTrue(StayLinked.wanted(on = true, features = true, access = true, paired = true))
        assertFalse(StayLinked.wanted(on = false, features = true, access = true, paired = true))
        assertFalse(StayLinked.wanted(on = true, features = false, access = true, paired = true))
        assertFalse(StayLinked.wanted(on = true, features = true, access = false, paired = true))
        assertFalse(StayLinked.wanted(on = true, features = true, access = true, paired = false))
    }

    @Test
    fun `the first reading is no freeze`() {
        assertFalse(FreezeWatch.frozen(previousUptime = null, uptime = 1_000_000))
    }

    @Test
    fun `ticks on time are no freeze`() {
        assertFalse(FreezeWatch.frozen(previousUptime = 100_000, uptime = 102_000))
        assertFalse(FreezeWatch.frozen(previousUptime = 100_000, uptime = 129_999))
    }

    @Test
    fun `thirty seconds of uptime between ticks is a freeze`() {
        assertTrue(FreezeWatch.frozen(previousUptime = 100_000, uptime = 130_000))
        assertTrue(FreezeWatch.frozen(previousUptime = 100_000, uptime = 700_000))
    }

    @Test
    fun `uptime going back is a reboot not a freeze`() {
        assertFalse(FreezeWatch.frozen(previousUptime = 900_000, uptime = 5_000))
    }

    @Test
    fun `a freeze counts for a day`() {
        val day = 24 * 60 * 60 * 1000L
        assertFalse(FreezeWatch.recent(lastFreezeMs = null, nowMs = 10 * day))
        assertTrue(FreezeWatch.recent(lastFreezeMs = 10 * day - 1000, nowMs = 10 * day))
        assertTrue(FreezeWatch.recent(lastFreezeMs = 9 * day + 1, nowMs = 10 * day))
        assertFalse(FreezeWatch.recent(lastFreezeMs = 9 * day - 1, nowMs = 10 * day))
    }

    @Test
    fun `a freeze dated ahead of the clock is not trusted`() {
        assertFalse(FreezeWatch.recent(lastFreezeMs = 2_000_000, nowMs = 1_000_000))
    }
}
