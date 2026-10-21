package dev.pocketdesk.android.media

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BackgroundReportTest {
    private val own = "127.0.0.1" to 1716

    private fun song(title: String, playing: Boolean = true, art: String? = null) = MediaRule.Media(
        title = title,
        artist = "Artist",
        app = "Music",
        packageName = "pkg.music",
        isPlaying = playing,
        canPlayPause = true,
        canNext = true,
        canPrevious = true,
        artId = art,
    )

    /** N2 review: a link lost while it plays is still wanted, so it is dialed again. */
    @Test
    fun aLinkLostWhilePlayingIsStillWanted() {
        val report = BackgroundReport()
        report.link(own, linked = true)
        report.see(song("One"), null)
        assertTrue(report.due(0))
        report.sent(0)
        report.link(null, linked = false)
        assertTrue("current", report.current(10_000))
        assertTrue("heard again once linked", report.link(own, linked = true) && report.due(10_000))
    }

    /** N2 review: A, then B, then A again sends A's cover again. */
    @Test
    fun aCoverGoesAgainWhenTheComputerNoLongerHoldsIt() {
        val report = BackgroundReport()
        report.link(own, linked = true)
        report.see(song("A", art = "aaaaaaaaaaaaaaaa"), null)
        assertTrue(report.withArt())
        report.sent(0)
        report.see(song("A", art = "aaaaaaaaaaaaaaaa"), null)
        assertFalse("held", report.withArt())
        report.see(song("B", art = "bbbbbbbbbbbbbbbb"), null)
        assertTrue(report.withArt())
        report.sent(1)
        report.see(song("A", art = "aaaaaaaaaaaaaaaa"), null)
        assertTrue("the computer let A's go", report.withArt())
        report.sent(2)
        report.see(null, null)
        report.sent(3)
        assertNull(report.heldArt)
        report.see(song("A", art = "aaaaaaaaaaaaaaaa"), null)
        assertTrue("after {}", report.withArt())
    }

    /** N2 review: a pause from long ago is not current, nor heard by a new link. */
    @Test
    fun aLongPauseIsNotCurrent() {
        val report = BackgroundReport()
        report.link(own, linked = true)
        report.see(song("One", playing = false), pausedSinceMs = 0)
        assertFalse(report.current(MediaRule.PAUSED_MS))
        assertFalse(report.due(MediaRule.PAUSED_MS))
        report.see(song("One"), pausedSinceMs = 0)
        assertNull("playing has no pause", report.pausedSinceMs)
        assertTrue(report.current(MediaRule.PAUSED_MS))
    }

    @Test
    fun anotherComputerOrNoneForgetsWhatWasSent() {
        val report = BackgroundReport()
        report.link(own, linked = true)
        report.see(song("One", art = "aaaaaaaaaaaaaaaa"), null)
        report.sent(0)
        assertFalse(report.link(own, linked = true))
        assertEquals("aaaaaaaaaaaaaaaa", report.heldArt)
        assertTrue(report.link("10.0.0.2" to 1716, linked = false))
        assertNull(report.heldArt)
        assertFalse(report.sent.any)
        report.forget()
        assertNull(report.seen)
    }
}
