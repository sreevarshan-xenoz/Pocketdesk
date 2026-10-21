package dev.pocketdesk.android.media

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerMediaRuleTest {
    private fun song(title: String = "One", playing: Boolean = true) = ComputerMediaRule.Playing(
        title = title,
        artist = "Artist",
        app = "Spotify",
        isPlaying = playing,
        canPlayPause = true,
        canNext = true,
        canPrevious = false,
    )

    @Test
    fun whatPlaysShowsUntilTheComputerGoesQuiet() {
        val shown = ComputerMediaRule.Shown()
        assertNull(shown.current(0))
        shown.heard(song(), 1_000)
        assertEquals("One", shown.current(1_000)?.title)
        assertEquals("One", shown.current(1_000 + ComputerMediaRule.SILENT_MS - 1)?.title)
        assertNull("no push for 150 s", shown.current(1_000 + ComputerMediaRule.SILENT_MS))
    }

    @Test
    fun nothingPlayingEndsAfterAMoment() {
        val shown = ComputerMediaRule.Shown()
        shown.heard(song(), 0)
        shown.heard(null, 2_000)
        shown.heard(null, 4_000)
        assertEquals("a gap between tracks", "One", shown.current(2_000 + ComputerMediaRule.NOTHING_MS - 1)?.title)
        assertNull(shown.current(2_000 + ComputerMediaRule.NOTHING_MS))
        // The next track within the moment shows at once.
        shown.heard(null, 20_000)
        shown.heard(song("Two"), 21_000)
        assertEquals("Two", shown.current(21_000)?.title)
        assertEquals("Two", shown.current(35_000)?.title)
    }

    @Test
    fun aPauseShowsFiveMinutesFromWhenItBegan() {
        val shown = ComputerMediaRule.Shown()
        shown.heard(song(), 0)
        shown.heard(song(playing = false), 10_000)
        // The minute's word again keeps the pause's start.
        shown.heard(song(playing = false), 70_000)
        shown.heard(song(playing = false), 130_000)
        shown.heard(song(playing = false), 250_000)
        assertEquals("One", shown.current(10_000 + ComputerMediaRule.PAUSED_MS - 1)?.title)
        assertNull(shown.current(10_000 + ComputerMediaRule.PAUSED_MS))
        // Playing again shows again.
        shown.heard(song(), 400_000)
        assertEquals("One", shown.current(400_000)?.title)
    }

    @Test
    fun clearedShowsNothing() {
        val shown = ComputerMediaRule.Shown()
        shown.heard(song(), 0)
        shown.clear()
        assertNull(shown.current(0))
    }

    @Test
    fun playAndPauseToggleOnlyWhenTheyChangeWhatShows() {
        assertTrue(ComputerMediaRule.toggles(wantPlaying = true, shownPlaying = false))
        assertFalse(ComputerMediaRule.toggles(wantPlaying = true, shownPlaying = true))
        assertTrue(ComputerMediaRule.toggles(wantPlaying = false, shownPlaying = true))
        assertFalse(ComputerMediaRule.toggles(wantPlaying = false, shownPlaying = false))
        assertTrue(ComputerMediaRule.toggles(wantPlaying = false, shownPlaying = null))
    }

    @Test
    fun onlyTheOwnComputersPushShows() {
        assertTrue(ComputerMediaRule.fromOwn("192.0.2.5", 1716, "192.0.2.5", 1716))
        assertFalse(ComputerMediaRule.fromOwn("192.0.2.6", 1716, "192.0.2.5", 1716))
        assertFalse(ComputerMediaRule.fromOwn("192.0.2.5", 1717, "192.0.2.5", 1716))
        assertFalse(ComputerMediaRule.fromOwn("192.0.2.5", 1716, null, null))
    }

    @Test
    fun aCoverIdIsSixteenLowercaseHexDigits() {
        assertEquals("ba7816bf8f01cfea", ComputerMediaRule.artId("ba7816bf8f01cfea"))
        assertNull(ComputerMediaRule.artId("BA7816BF8F01CFEA"))
        assertNull(ComputerMediaRule.artId("ba7816bf"))
        assertNull(ComputerMediaRule.artId("../../etc/passwd"))
        assertNull(ComputerMediaRule.artId(null))
    }

    @Test
    fun eachLinkIsAskedOnceAndToldToStopOnlyWhereAsked() {
        val watch = ComputerMediaRule.Watch()
        assertNull("no link", watch.due(null, wanted = true))
        assertNull("never asked: nothing to stop", watch.due("a", wanted = false))
        assertEquals(true, watch.due("a", wanted = true))
        watch.sent("a", true)
        assertNull("asked already", watch.due("a", wanted = true))
        assertEquals(false, watch.due("a", wanted = false))
        watch.sent("a", false)
        assertNull(watch.due("a", wanted = false))
        assertEquals("asked again", true, watch.due("a", wanted = true))
        watch.sent("a", true)
        // A new link is asked again; one that dropped forgets.
        assertEquals(true, watch.due("b", wanted = true))
        assertNull(watch.due(null, wanted = true))
        assertEquals(true, watch.due("b", wanted = true))
    }

    @Test
    fun aWatchUnansweredOrMissingItsCoverIsAskedAgainAtMostEveryThirtySeconds() {
        val watch = ComputerMediaRule.Watch()
        assertEquals(true, watch.due("a", wanted = true, nowMs = 0))
        watch.sent("a", true, nowMs = 0)
        // Pushes come: nothing to ask.
        assertNull(watch.due("a", wanted = true, nowMs = 60_000, heardMs = 60_000))
        // Quiet for 75 s since the last push.
        assertNull(watch.due("a", wanted = true, nowMs = 134_999, heardMs = 60_000))
        assertEquals(true, watch.due("a", wanted = true, nowMs = 135_000, heardMs = 60_000))
        watch.sent("a", true, nowMs = 135_000)
        // Asked: quiet is counted from the ask, and not again within 30 s.
        assertNull(watch.due("a", wanted = true, nowMs = 140_000, heardMs = 60_000, coverMissing = true))
        assertEquals(true, watch.due("a", wanted = true, nowMs = 165_000, heardMs = 160_000, coverMissing = true))
        // Never while not wanted.
        watch.sent("a", false, nowMs = 165_000)
        assertNull(watch.due("a", wanted = false, nowMs = 999_000, coverMissing = true))
    }
}
