package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LiveRuleTest {
    private fun read(
        ongoing: Boolean = true,
        own: Boolean = false,
        media: Boolean = false,
        call: Boolean = false,
        progress: Int = 0,
        max: Int = 0,
        indeterminate: Boolean = false,
        showChronometer: Boolean = false,
        whenMs: Long = 0,
        countdown: Boolean = false,
    ) = LiveRule.read(ongoing, own, media, call, progress, max, indeterminate, showChronometer, whenMs, countdown)

    @Test
    fun onlyAnOngoingBarOrTimerIsLive() {
        assertEquals(42, read(progress = 42, max = 100)?.percent)
        assertEquals(1_000L, read(showChronometer = true, whenMs = 1_000)?.chronometer)
        assertEquals(null, read(indeterminate = true)?.percent)
        assertNull("not ongoing", read(ongoing = false, progress = 1, max = 2))
        assertNull("its own", read(own = true, progress = 1, max = 2))
        assertNull("a player's", read(media = true, progress = 1, max = 2))
        assertNull("a call's", read(call = true, showChronometer = true, whenMs = 1))
        assertNull("a service's note with neither", read())
        assertNull("a timer with no base", read(showChronometer = true))
    }

    @Test
    fun anUpdateWaitsForAStepOrAChangeAndTheGap() {
        val at = { p: Int -> LiveRule.Live(p.toLong(), 100, false, null, false) }
        assertEquals("first at once", 0L, LiveRule.due(null, at(10), "Down", "x", 0))
        val last = LiveRule.Sent(1_000, at(10), "Down", "x")
        assertNull("a small move", LiveRule.due(last, at(12), "Down", "x", 9_000))
        assertEquals("a step, after the gap", 0L, LiveRule.due(last, at(15), "Down", "x", 6_000))
        assertEquals("a step, inside the gap", 3_000L, LiveRule.due(last, at(15), "Down", "x", 3_000))
        assertEquals("new words wait too", 2_000L, LiveRule.due(last, at(10), "Done soon", "x", 4_000))
        assertEquals("finished never waits", 0L, LiveRule.due(last, at(100), "Down", "x", 1_001))
        val done = LiveRule.Sent(1_000, at(100), "Down", "x")
        assertNull("finished once", LiveRule.due(done, at(100), "Down", "x", 9_000))
        val timer = LiveRule.Live(0, 0, false, 5_000, false)
        val lastTimer = LiveRule.Sent(1_000, timer, "Timer", "")
        assertNull("a running timer says nothing new", LiveRule.due(lastTimer, timer, "Timer", "", 60_000))
        assertEquals("a reset timer", 0L, LiveRule.due(lastTimer, timer.copy(chronometer = 9_000), "Timer", "", 60_000))
    }
}
