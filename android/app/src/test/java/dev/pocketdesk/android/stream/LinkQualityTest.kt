package dev.pocketdesk.android.stream

import dev.pocketdesk.android.stream.LinkQuality.Level
import org.junit.Assert.assertEquals
import org.junit.Test

class LinkQualityTest {
    @Test
    fun nothingMeasuredYetIsNotAVerdict() {
        assertEquals(Level.MEASURING, LinkQuality.of(rttMs = 0, rttSamples = 0, e2eMs = 0, recentDrops = 0))
    }

    @Test
    fun aQuickLinkIsGood() {
        assertEquals(Level.GOOD, LinkQuality.of(rttMs = 12, rttSamples = 3, e2eMs = 40, recentDrops = 0))
        // No video yet: the round trip alone decides.
        assertEquals(Level.GOOD, LinkQuality.of(rttMs = 30, rttSamples = 1, e2eMs = 0, recentDrops = 0))
    }

    @Test
    fun aSlowRoundTripOrDelayIsFairThenPoor() {
        assertEquals(Level.FAIR, LinkQuality.of(rttMs = 120, rttSamples = 8, e2eMs = 90, recentDrops = 0))
        assertEquals(Level.FAIR, LinkQuality.of(rttMs = 40, rttSamples = 8, e2eMs = 180, recentDrops = 0))
        assertEquals(Level.POOR, LinkQuality.of(rttMs = 260, rttSamples = 8, e2eMs = 90, recentDrops = 0))
        assertEquals(Level.POOR, LinkQuality.of(rttMs = 40, rttSamples = 8, e2eMs = 320, recentDrops = 0))
    }

    @Test
    fun framesLostLatelyLowerIt() {
        assertEquals(Level.FAIR, LinkQuality.of(rttMs = 12, rttSamples = 8, e2eMs = 40, recentDrops = 2))
        assertEquals(Level.POOR, LinkQuality.of(rttMs = 12, rttSamples = 8, e2eMs = 40, recentDrops = 10))
    }

    @Test
    fun dropsCountOnlyOverTheRecentWindow() {
        val window = LinkQuality.DropWindow(spanMs = 3_000)
        window.add(atMs = 0, total = 50)
        window.add(atMs = 1_000, total = 50)
        window.add(atMs = 2_000, total = 51)
        assertEquals(1L, window.recent())
        // The drop at 2 s is older than the window by 6 s.
        window.add(atMs = 6_000, total = 51)
        assertEquals(0L, window.recent())
        // A new session counts from zero: never a negative count.
        window.add(atMs = 7_000, total = 0)
        assertEquals(0L, window.recent())
    }
}
