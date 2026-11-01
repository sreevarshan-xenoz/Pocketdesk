package dev.pocketdesk.android.windows

import dev.pocketdesk.android.windows.HotspotRule.Ask
import dev.pocketdesk.android.windows.HotspotRule.Decision
import dev.pocketdesk.android.windows.HotspotRule.Mode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HotspotRuleTest {
    @Test
    fun theHotspotIsOnlyOnWithShizukuItsPermissionAndTheOwnersSwitch() {
        assertEquals(Mode.ON, HotspotRule.mode(34, shizukuRunning = true, permitted = true, switchOn = true))
        // Off by default: nothing else makes it on.
        assertEquals(Mode.OFF, HotspotRule.mode(34, shizukuRunning = true, permitted = true, switchOn = false))
        assertEquals(Mode.NEEDS_PERMISSION, HotspotRule.mode(34, shizukuRunning = true, permitted = false, switchOn = true))
        assertEquals(Mode.NO_SHIZUKU, HotspotRule.mode(34, shizukuRunning = false, permitted = true, switchOn = true))
        assertEquals(Mode.TOO_OLD, HotspotRule.mode(HotspotRule.MIN_SDK - 1, true, true, true))
        assertEquals(Mode.ON, HotspotRule.mode(HotspotRule.MIN_SDK, true, true, true))
    }

    @Test
    fun theRowIsShownOnlyWithShizukuRunningOrWhileItsSwitchIsOn() {
        assertTrue(HotspotRule.rowShown(34, shizukuRunning = true, switchOn = false))
        assertFalse("nobody is told about Shizuku", HotspotRule.rowShown(34, shizukuRunning = false, switchOn = false))
        assertTrue("it can always be turned off", HotspotRule.rowShown(34, shizukuRunning = false, switchOn = true))
        assertFalse(HotspotRule.rowShown(HotspotRule.MIN_SDK - 1, shizukuRunning = true, switchOn = true))
    }

    @Test
    fun theComputerIsToldItOnlyWhenTheSwitchIsOnAndShizukuIsUsable() {
        val ready = HotspotRule.report(34, shizukuRunning = true, permitted = true, switchOn = true)
        assertTrue(ready.on)
        assertEquals(true, ready.android)
        val off = HotspotRule.report(34, shizukuRunning = true, permitted = true, switchOn = false)
        assertFalse(off.on)
        assertEquals(false, HotspotRule.report(34, shizukuRunning = false, permitted = true, switchOn = true).android)
        assertEquals(false, HotspotRule.report(34, shizukuRunning = true, permitted = false, switchOn = true).android)
        assertEquals(false, HotspotRule.report(HotspotRule.MIN_SDK - 1, true, true, true).android)
    }

    @Test
    fun anAskToTurnItOnIsDoneOnlyForTheOwnComputerWithTheSwitchOnAndShizuku() {
        assertEquals(Decision.DO, HotspotRule.decide(Ask.ON, Mode.ON, own = true, withinLimit = true))
        assertEquals(Decision.OFF, HotspotRule.decide(Ask.ON, Mode.OFF, own = true, withinLimit = true))
        assertEquals(Decision.NOT_OWN, HotspotRule.decide(Ask.ON, Mode.ON, own = false, withinLimit = true))
        assertEquals(Decision.UNAVAILABLE, HotspotRule.decide(Ask.ON, Mode.NO_SHIZUKU, own = true, withinLimit = true))
        assertEquals(Decision.UNAVAILABLE, HotspotRule.decide(Ask.ON, Mode.NEEDS_PERMISSION, own = true, withinLimit = true))
        assertEquals(Decision.UNAVAILABLE, HotspotRule.decide(Ask.ON, Mode.TOO_OLD, own = true, withinLimit = true))
        assertEquals(Decision.TOO_MANY, HotspotRule.decide(Ask.ON, Mode.ON, own = true, withinLimit = false))
    }

    @Test
    fun theComputerIsNeverAnsweredUnlessItIsTheOwnWhateverElseIsTrue() {
        for (ask in Ask.entries) for (mode in Mode.entries) for (limit in listOf(true, false)) {
            assertEquals(
                "$ask $mode",
                Decision.NOT_OWN,
                HotspotRule.decide(ask, mode, own = false, withinLimit = limit),
            )
        }
    }

    @Test
    fun anAskToTurnItOffStillGoesWithTheSwitchOffSoNoHotspotIsLeftBehind() {
        assertEquals(Decision.DO, HotspotRule.decide(Ask.OFF, Mode.OFF, own = true, withinLimit = true))
        assertEquals(Decision.DO, HotspotRule.decide(Ask.OFF, Mode.ON, own = true, withinLimit = true))
        // But never without Shizuku, never for another computer, and it counts against the limit.
        assertEquals(Decision.UNAVAILABLE, HotspotRule.decide(Ask.OFF, Mode.NO_SHIZUKU, own = true, withinLimit = true))
        assertEquals(Decision.NOT_OWN, HotspotRule.decide(Ask.OFF, Mode.OFF, own = false, withinLimit = true))
        assertEquals(Decision.TOO_MANY, HotspotRule.decide(Ask.OFF, Mode.OFF, own = true, withinLimit = false))
    }

    @Test
    fun theRateLimitHoldsWithinItsWindowAndFreesAfterIt() {
        val limiter = HotspotRule.Limiter(max = 3, windowMs = 1000)
        assertTrue(limiter.take(0))
        assertTrue(limiter.take(100))
        assertTrue(limiter.take(200))
        assertFalse("the fourth in a window", limiter.take(300))
        assertFalse(limiter.take(999))
        assertTrue("the first has left the window", limiter.take(1000))
        assertFalse(limiter.take(1001))
    }

    @Test
    fun theDefaultLimitIsSmallAndSlow() {
        assertEquals(8, HotspotRule.LIMIT_ASKS)
        assertEquals(10 * 60_000L, HotspotRule.LIMIT_WINDOW_MS)
    }

    @Test
    fun theStopCommandIsFixedWordsWithNoShellAndNothingFromACaller() {
        // The one command the helper ever runs for this: absolute path, fixed words.
        assertEquals(listOf("/system/bin/cmd", "wifi", "stop-softap"), HotspotRule.STOP_ARGV)
        // No shell, no way to name a network, a password or a band.
        for (word in HotspotRule.STOP_ARGV) {
            assertFalse(word, word.contains(' ') || word.contains(';') || word.contains('$') || word.contains('|'))
        }
        assertFalse(HotspotRule.STOP_ARGV.any { it == "sh" || it == "-c" || it == "start-softap" || it == "start-lohs" })
    }

    @Test
    fun theHotspotIsTurnedOffByItselfOnlyAfterTheOwnersMinutes() {
        val start = 1_000_000L
        assertFalse(HotspotRule.offDue(start, start + 29 * 60_000L, 30))
        assertTrue(HotspotRule.offDue(start, start + 30 * 60_000L, 30))
        assertFalse("never means never", HotspotRule.offDue(start, start + 10_000 * 60_000L, 0))
        assertFalse("none is on", HotspotRule.offDue(0L, start, 30))
    }

    @Test
    fun theAutoOffChoicesGoRoundAndTheDefaultIsOne() {
        assertTrue(HotspotRule.DEFAULT_OFF_MINUTES in HotspotRule.OFF_CHOICES)
        var at = HotspotRule.OFF_CHOICES.first()
        val seen = mutableListOf(at)
        repeat(HotspotRule.OFF_CHOICES.size) {
            at = HotspotRule.nextChoice(at)
            seen += at
        }
        assertEquals(seen.first(), seen.last())
        assertEquals(HotspotRule.OFF_CHOICES.toSet(), seen.toSet())
        // A value that is not a choice (an old setting) lands on the first.
        assertEquals(HotspotRule.OFF_CHOICES.first(), HotspotRule.nextChoice(-5))
    }
}
