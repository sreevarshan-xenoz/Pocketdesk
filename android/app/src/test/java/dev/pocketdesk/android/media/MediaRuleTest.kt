package dev.pocketdesk.android.media

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MediaRuleTest {
    private fun media(app: String, playing: Boolean) = MediaRule.Media(
        title = "Song",
        artist = "Artist",
        app = app,
        packageName = "pkg.$app",
        isPlaying = playing,
        canPlayPause = true,
        canNext = true,
        canPrevious = true,
    )

    /** N2: what plays is said again each minute, and a pause for five minutes, then left. */
    @Test
    fun theBackgroundReporterSaysAgainWhatIsCurrent() {
        val playing = media("Spotify", playing = true)
        val paused = playing.copy(isPlaying = false)
        assertTrue("first", MediaRule.backgroundDue(MediaRule.Sent(), playing, 0, null))
        assertFalse("nothing, never sent", MediaRule.backgroundDue(MediaRule.Sent(), null, 0, null))
        val sent = MediaRule.Sent(any = true, last = playing, atMs = 1_000)
        assertFalse(MediaRule.backgroundDue(sent, playing, 1_000 + 59_999, null))
        assertTrue(MediaRule.backgroundDue(sent, playing, 1_000 + MediaRule.RESEND_MS, null))
        assertTrue("a pause is a change", MediaRule.backgroundDue(sent, paused, 2_000, 2_000))
        val sentPaused = MediaRule.Sent(any = true, last = paused, atMs = 2_000)
        assertTrue(MediaRule.backgroundDue(sentPaused, paused, 2_000 + MediaRule.RESEND_MS, 2_000))
        assertFalse(
            "paused too long",
            MediaRule.backgroundDue(sentPaused, paused, 2_000 + MediaRule.PAUSED_MS, 2_000),
        )
        assertTrue("stopped: {} once", MediaRule.backgroundDue(sent, null, 3_000, null))
        val sentNothing = MediaRule.Sent(any = true, last = null, atMs = 3_000)
        assertFalse(MediaRule.backgroundDue(sentNothing, null, 3_000 + MediaRule.RESEND_MS, null))
        assertTrue("a cover that came late is a change", MediaRule.backgroundDue(sent, playing.copy(artId = "0123456789abcdef"), 1_001, null))
    }

    @Test
    fun aPlayerIsCurrentWhilePlayingOrPausedFiveMinutes() {
        val playing = media("Spotify", playing = true)
        val paused = playing.copy(isPlaying = false)
        assertTrue(MediaRule.current(playing, 0, null))
        assertTrue(MediaRule.current(paused, MediaRule.PAUSED_MS - 1, 0))
        assertFalse(MediaRule.current(paused, MediaRule.PAUSED_MS, 0))
        assertFalse(MediaRule.current(paused, 0, null))
        assertFalse(MediaRule.current(null, 0, 0))
    }

    @Test
    fun aCoverGoesOncePerLink() {
        assertTrue(MediaRule.sendArt("0123456789abcdef", emptySet()))
        assertFalse(MediaRule.sendArt("0123456789abcdef", setOf("0123456789abcdef")))
        assertFalse(MediaRule.sendArt(null, emptySet()))
    }

    @Test
    fun thePlayingSessionIsReportedElseTheMostRecent() {
        val paused = media("Podcasts", playing = false)
        val playing = media("Spotify", playing = true)
        // Android lists the most recently active first.
        assertEquals(playing, MediaRule.pick(listOf(paused, playing)))
        assertEquals(paused, MediaRule.pick(listOf(paused, media("Radio", playing = false))))
        assertNull(MediaRule.pick(emptyList()))
    }

    @Test
    fun aReportGoesOnAChangeAndNothingIsSaidOnceWhenItStops() {
        val song = media("Spotify", playing = true)
        val s = MediaRule.Sent()
        assertFalse("nothing to clear at the start", MediaRule.shouldSend(s, null))
        assertTrue(MediaRule.shouldSend(s, song))
        val sent = MediaRule.Sent(any = true, last = song)
        assertFalse("the same", MediaRule.shouldSend(sent, song))
        assertTrue("paused", MediaRule.shouldSend(sent, song.copy(isPlaying = false)))
        assertTrue("stopped: {} once", MediaRule.shouldSend(sent, null))
        assertFalse(MediaRule.shouldSend(MediaRule.Sent(any = true, last = null), null))
    }

    @Test
    fun whatPlaysIsSaidAgainEachMinuteSoAComputerThatDroppedItHearsIt() {
        // The computer forgets a song when Media is turned off; turned back
        // on, it hears the song again within a minute, not only on a change.
        val song = media("Spotify", playing = true)
        val sent = MediaRule.Sent(any = true, last = song, atMs = 1_000)
        assertFalse(MediaRule.shouldSend(sent, song, nowMs = 1_000 + MediaRule.RESEND_MS - 1))
        assertTrue(MediaRule.shouldSend(sent, song, nowMs = 1_000 + MediaRule.RESEND_MS))
        // Nothing playing is not said again: {} once is enough.
        val nothing = MediaRule.Sent(any = true, last = null, atMs = 1_000)
        assertFalse(MediaRule.shouldSend(nothing, null, nowMs = 1_000 + 10 * MediaRule.RESEND_MS))
    }

    @Test
    fun aLostLinkForgetsWhatWasSentSoTheNextLinkHearsItAgain() {
        val song = media("Spotify", playing = true)
        val sent = MediaRule.Sent(any = true, last = song)
        assertEquals(sent, MediaRule.forLink(sent, linked = true))
        // A computer restarted, or reached again, knows nothing: say it again.
        val afresh = MediaRule.forLink(sent, linked = false)
        assertTrue(MediaRule.shouldSend(afresh, song))
    }

    @Test
    fun aPressActsOnlyWithTheSettingOnAndOnlyForTheThreeCommands() {
        assertTrue(MediaRule.accepts("playPause", settingOn = true))
        assertTrue(MediaRule.accepts("next", settingOn = true))
        assertTrue(MediaRule.accepts("previous", settingOn = true))
        assertFalse(MediaRule.accepts("next", settingOn = false))
        assertFalse(MediaRule.accepts("stop", settingOn = true))
    }
}
