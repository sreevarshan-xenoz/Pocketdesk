package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.NowPlaying
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MediaWidgetPlanTest {
    private fun playing(isPlaying: Boolean = true, previous: Boolean = true, playPause: Boolean = true, next: Boolean = false) =
        NowPlaying("Song", "Artist", "Spotify.exe", isPlaying, playPause, next, previous)

    @Test
    fun theButtonsAreWhatThePlayerOffers() {
        val b = MediaWidgetPlan.buttons(playing())
        assertTrue(b.previous)
        assertTrue(b.playPause)
        assertFalse(b.next)
        val none = MediaWidgetPlan.buttons(null)
        assertFalse(none.previous || none.playPause || none.next)
    }

    @Test
    fun playPauseShowsWhatATapDoes() {
        assertEquals(MediaWidgetPlan.Toggle.Pause, MediaWidgetPlan.toggle(playing(isPlaying = true)))
        assertEquals(MediaWidgetPlan.Toggle.Play, MediaWidgetPlan.toggle(playing(isPlaying = false)))
        assertEquals(MediaWidgetPlan.Toggle.Play, MediaWidgetPlan.toggle(null))
    }

    @Test
    fun theComputerIsAskedWhenAskedOrNeverTried() {
        assertTrue(MediaWidgetPlan.ask(MediaWidgetPlan.Why.Refresh, tried = true))
        assertTrue(MediaWidgetPlan.ask(MediaWidgetPlan.Why.Follow, tried = false))
        assertFalse(MediaWidgetPlan.ask(MediaWidgetPlan.Why.Follow, tried = true))
        assertFalse(MediaWidgetPlan.ask(MediaWidgetPlan.Why.Period, tried = true))
    }

    @Test
    fun onlyTheCommandsTheComputerKnowsAreSent() {
        assertEquals("playPause", MediaWidgetPlan.command(MediaWidgetPlan.Button.PlayPause))
        assertEquals("next", MediaWidgetPlan.command(MediaWidgetPlan.Button.Next))
        assertEquals("previous", MediaWidgetPlan.command(MediaWidgetPlan.Button.Previous))
        assertEquals(MediaWidgetPlan.Button.Next, MediaWidgetPlan.button("next"))
        assertEquals(null, MediaWidgetPlan.button("volumeUp"))
    }

    /** I2: the track with its buttons, or one message in the widget's middle. */
    @Test
    fun theBodyIsTheTrackOrWhatThereIsInstead() {
        val track = MediaWidgetPlan.body(hasComputer = true, playing = playing(), answered = true, failed = false, asking = false)
        assertEquals(MediaWidgetPlan.Body.Track, track)
        // Asking again keeps the track it has.
        assertEquals(
            MediaWidgetPlan.Body.Track,
            MediaWidgetPlan.body(hasComputer = true, playing = playing(), answered = true, failed = false, asking = true),
        )
        assertEquals(
            MediaWidgetPlan.Body.Nothing,
            MediaWidgetPlan.body(hasComputer = true, playing = null, answered = true, failed = false, asking = false),
        )
        assertEquals(
            MediaWidgetPlan.Body.Failed,
            MediaWidgetPlan.body(hasComputer = true, playing = null, answered = false, failed = true, asking = false),
        )
        assertEquals(
            MediaWidgetPlan.Body.Asking,
            MediaWidgetPlan.body(hasComputer = true, playing = null, answered = false, failed = true, asking = true),
        )
        assertEquals(
            MediaWidgetPlan.Body.Asking,
            MediaWidgetPlan.body(hasComputer = true, playing = null, answered = false, failed = false, asking = false),
        )
        assertEquals(
            MediaWidgetPlan.Body.NoComputer,
            MediaWidgetPlan.body(hasComputer = false, playing = null, answered = false, failed = false, asking = false),
        )
    }

    @Test
    fun anAnswerIsKeptOnlyForItsComputer() {
        assertTrue(MediaWidgetPlan.keeps(readFor = "a", lastComputer = "a"))
        assertFalse(MediaWidgetPlan.keeps(readFor = "a", lastComputer = "b"))
        assertFalse(MediaWidgetPlan.keeps(readFor = null, lastComputer = "a"))
    }
}
