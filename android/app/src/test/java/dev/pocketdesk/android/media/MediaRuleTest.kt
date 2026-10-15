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
