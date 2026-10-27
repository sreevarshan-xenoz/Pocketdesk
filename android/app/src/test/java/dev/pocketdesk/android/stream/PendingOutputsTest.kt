package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PendingOutputsTest {
    private fun pending() = PendingOutputs(staleUs = 100_000, max = 8)

    @Test
    fun aPictureIsDueUntilItComesOut() {
        val p = pending()
        assertFalse(p.due(now = 0))
        p.fed(at = 1_000)
        assertTrue(p.due(now = 2_000))
        assertEquals(1_000L, p.out())
        assertFalse(p.due(now = 3_000))
    }

    @Test
    fun aFrameTheDecoderDroppedIsNotDueForever() {
        val p = pending()
        p.fed(at = 0)
        // Long past any decode time: dropped, so the polling goes back to normal.
        assertFalse(p.due(now = 150_000))
        assertEquals(0, p.size)
    }

    @Test
    fun picturesPairWithTheirFeedsInOrder() {
        val p = pending()
        p.fed(at = 10)
        p.fed(at = 20)
        assertEquals(10L, p.out())
        assertEquals(20L, p.out())
        assertNull(p.out())
    }

    @Test
    fun aLostPairingKeepsOnlyTheRecentFeeds() {
        val p = PendingOutputs(staleUs = 100_000, max = 2)
        p.fed(at = 1)
        p.fed(at = 2)
        p.fed(at = 3)
        assertEquals(2, p.size)
        assertEquals(2L, p.out())
    }
}
