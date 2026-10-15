package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class NowPlayingTest {
    @Test
    fun theAppIsNamedAsAPersonKnowsIt() {
        assertEquals("Spotify", NowPlaying.appName("Spotify.exe"))
        assertEquals("ZuneMusic", NowPlaying.appName("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"))
        assertEquals("Chrome", NowPlaying.appName("Chrome"))
        assertEquals("firefox", NowPlaying.appName("firefox"))
        assertEquals("", NowPlaying.appName(""))
    }

    @Test
    fun theCardSaysTheTitleThenWhoAndWhere() {
        val song = NowPlaying(
            title = "Song",
            artist = "Artist",
            app = "Spotify.exe",
            isPlaying = true,
            canPlayPause = true,
            canNext = true,
            canPrevious = false,
        )
        assertEquals("Song", song.headline())
        assertEquals("Artist · Spotify", song.subline())
        // A video has no artist; with no title the app stands in.
        assertEquals("Chrome", song.copy(title = " ", artist = "", app = "Chrome").headline())
        assertEquals("", song.copy(title = " ", artist = "", app = "Chrome").subline())
        assertEquals("Song", song.copy(artist = "", app = "").headline())
        assertEquals("", song.copy(artist = "", app = "").subline())
    }
}
