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

    private val minute = 60_000L
    private val run = HotspotRule.Run(firstOnAtMs = 1_000L, lastOnAtMs = 1_000L, asker = "192.0.2.7")

    @Test
    fun aStartThatStartedOneBeginsARunAndALaterAskNeverMovesItsFirstTime() {
        val first = HotspotRule.afterOn(null, HotspotRule.RESULT_DONE, "192.0.2.7", 1_000L)
        assertEquals(HotspotRule.Run(1_000L, 1_000L, "192.0.2.7"), first)
        // Asked again an hour later: renewed for the minutes, but the cap still counts from the first.
        val again = HotspotRule.afterOn(first, HotspotRule.RESULT_DONE, "192.0.2.7", 1_000L + 60 * minute)
        assertEquals(1_000L, again!!.firstOnAtMs)
        assertEquals(1_000L + 60 * minute, again.lastOnAtMs)
    }

    @Test
    fun aHotspotTheOwnerTurnedOnThemselvesIsNeverRecordedSoNeverTurnedOffFromHere() {
        // A start that found one running starts no run...
        assertEquals(null, HotspotRule.afterOn(null, HotspotRule.RESULT_ALREADY_ON, "192.0.2.7", 5_000L))
        // ...but one of ours that is still running is renewed, keeping its first time.
        val renewed = HotspotRule.afterOn(run, HotspotRule.RESULT_ALREADY_ON, "192.0.2.7", 9_000L)
        assertEquals(HotspotRule.Run(1_000L, 9_000L, "192.0.2.7"), renewed)
        // A failed start changes nothing, with or without a run.
        assertEquals(null, HotspotRule.afterOn(null, HotspotRule.RESULT_FAILED, "192.0.2.7", 5_000L))
        assertEquals(run, HotspotRule.afterOn(run, HotspotRule.RESULT_FAILED, "192.0.2.7", 5_000L))
    }

    @Test
    fun onlyWifiAccessPointsCountAsAHotspotRunning() {
        assertTrue(HotspotRule.wifiApAmong(listOf("ap0")))
        assertTrue(HotspotRule.wifiApAmong(listOf("rndis0", "swlan0")))
        assertTrue(HotspotRule.wifiApAmong(listOf("wlan1")))
        assertTrue(HotspotRule.wifiApAmong(listOf("softap0")))
        assertFalse("a USB tether is not a hotspot", HotspotRule.wifiApAmong(listOf("rndis0", "usb0")))
        assertFalse("a Bluetooth tether is not one", HotspotRule.wifiApAmong(listOf("bt-pan")))
        assertFalse(HotspotRule.wifiApAmong(emptyList()))
        assertFalse(HotspotRule.wifiApAmong(listOf("wlan", "xap0", "ap0x")))
    }

    @Test
    fun theCapTurnsItOffWhateverTheSettingIncludingNever() {
        val capped = 1_000L + HotspotRule.MAX_LIFETIME_MS
        for (minutes in HotspotRule.OFF_CHOICES) {
            assertEquals(
                "minutes $minutes",
                HotspotRule.StopWhy.CAP,
                HotspotRule.stopDue(run, capped, minutes, askerPaired = true, linkLostForMs = 0),
            )
            assertEquals(
                null,
                HotspotRule.stopDue(run.copy(lastOnAtMs = capped - 1), capped - 1, 0, askerPaired = true, linkLostForMs = 0),
            )
        }
        assertEquals(4 * 60 * minute, HotspotRule.MAX_LIFETIME_MS)
    }

    @Test
    fun aLaterAskDoesNotExtendPastTheCap() {
        // First on at 1_000, asked again just before the cap: renewed, yet the cap is from the first.
        val late = 1_000L + HotspotRule.MAX_LIFETIME_MS - minute
        val renewed = HotspotRule.afterOn(run, HotspotRule.RESULT_DONE, "192.0.2.7", late)
        assertEquals(null, HotspotRule.stopDue(renewed, late, 0, true, 0))
        assertEquals(
            HotspotRule.StopWhy.CAP,
            HotspotRule.stopDue(renewed, late + minute, 0, true, 0),
        )
    }

    @Test
    fun theAskingComputerGoingAwayTurnsItOff() {
        // Unpaired or revoked: at once, whatever else is true.
        assertEquals(HotspotRule.StopWhy.ASKER_GONE, HotspotRule.stopDue(run, 2_000L, 0, askerPaired = false, linkLostForMs = 0))
        // Its link gone: only after the grace.
        assertEquals(null, HotspotRule.stopDue(run, 2_000L, 0, true, HotspotRule.LINK_GRACE_MS - 1))
        assertEquals(HotspotRule.StopWhy.LINK_LOST, HotspotRule.stopDue(run, 2_000L, 0, true, HotspotRule.LINK_GRACE_MS))
        assertEquals(2 * minute, HotspotRule.LINK_GRACE_MS)
    }

    @Test
    fun theOwnersMinutesStillTurnItOffAndNeverMeansNoTimer() {
        assertEquals(HotspotRule.StopWhy.TIMER, HotspotRule.stopDue(run, 1_000L + 30 * minute, 30, true, 0))
        assertEquals(null, HotspotRule.stopDue(run, 1_000L + 29 * minute, 30, true, 0))
        assertEquals(null, HotspotRule.stopDue(run, 1_000L + 3 * 60 * minute, 0, true, 0))
        assertEquals(null, HotspotRule.stopDue(null, 1_000L + 99 * 60 * minute, 30, false, 99 * minute))
    }

    @Test
    fun aStopThatFailedIsTriedAgainOnlyAfterTheRetryTime() {
        assertTrue("never tried (a real clock is far past the first time)", HotspotRule.retryDue(0L, 100_000L))
        assertFalse(HotspotRule.retryDue(10_000L, 10_000L + HotspotRule.RETRY_MS - 1))
        assertTrue(HotspotRule.retryDue(10_000L, 10_000L + HotspotRule.RETRY_MS))
        assertTrue("a clock set back does not stop the tries", HotspotRule.retryDue(10_000L, 5_000L))
    }

    @Test
    fun theLinkWatchCountsFromTheFirstTimeItSawTheLinkGoneAndResetsWhenItIsBack() {
        val watch = HotspotRule.LinkWatch()
        assertEquals(0L, watch.lostFor(linked = true, nowMs = 1_000L))
        assertEquals(0L, watch.lostFor(linked = false, nowMs = 2_000L))
        assertEquals(30_000L, watch.lostFor(linked = false, nowMs = 32_000L))
        assertEquals(0L, watch.lostFor(linked = true, nowMs = 40_000L))
        assertEquals(0L, watch.lostFor(linked = false, nowMs = 50_000L))
        assertEquals(HotspotRule.LINK_GRACE_MS, watch.lostFor(linked = false, nowMs = 50_000L + HotspotRule.LINK_GRACE_MS))
    }

    @Test
    fun aStopHeldThroughFailuresIsNeverForgottenByTheRule() {
        // The run is only ever replaced by afterOn (an ask), never cleared by a reason: a reason leaves
        // it for the caller, which clears it only when the helper says the stop went through.
        val after = HotspotRule.afterOn(run, HotspotRule.RESULT_FAILED, "192.0.2.7", 10 * minute)
        assertEquals(run, after)
        assertEquals(HotspotRule.StopWhy.CAP, HotspotRule.stopDue(after, 1_000L + HotspotRule.MAX_LIFETIME_MS + 7 * minute, 30, true, 0))
    }
}
