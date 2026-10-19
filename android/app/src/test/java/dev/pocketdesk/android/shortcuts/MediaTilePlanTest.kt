package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.NowPlaying
import org.junit.Assert.assertEquals
import org.junit.Test

class MediaTilePlanTest {
    private fun playing(isPlaying: Boolean) = NowPlaying("Song", "Artist", "Spotify.exe", isPlaying, true, true, true)

    @Test
    fun theTileIsOnWhileTheComputerPlays() {
        assertEquals(MediaTilePlan.Look.Playing, MediaTilePlan.look(hasComputer = true, playing = playing(true), failed = false, asking = false))
        assertEquals(MediaTilePlan.Look.Paused, MediaTilePlan.look(hasComputer = true, playing = playing(false), failed = false, asking = false))
        assertEquals(MediaTilePlan.Look.Unknown, MediaTilePlan.look(hasComputer = true, playing = null, failed = false, asking = false))
        assertEquals(MediaTilePlan.Look.Failed, MediaTilePlan.look(hasComputer = true, playing = playing(true), failed = true, asking = false))
        assertEquals(MediaTilePlan.Look.Asking, MediaTilePlan.look(hasComputer = true, playing = playing(true), failed = false, asking = true))
        assertEquals(MediaTilePlan.Look.NoComputer, MediaTilePlan.look(hasComputer = false, playing = playing(true), failed = false, asking = true))
    }

    @Test
    fun onlyPlayingShowsTheTileOn() {
        assertEquals(true, MediaTilePlan.Look.Playing.on)
        assertEquals(false, MediaTilePlan.Look.Paused.on)
        assertEquals(false, MediaTilePlan.Look.Failed.on)
    }
}
