package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Test

class TransferProgressTest {
    @Test
    fun percentIsWholeAndBounded() {
        assertEquals(0, TransferProgress.percent(0, 100))
        assertEquals(50, TransferProgress.percent(50, 100))
        assertEquals(99, TransferProgress.percent(999, 1000))
        assertEquals(100, TransferProgress.percent(100, 100))
        assertEquals(100, TransferProgress.percent(150, 100))
        assertEquals("an unknown total", 0, TransferProgress.percent(10, 0))
    }

    @Test
    fun theSpeedIsWhatMovedOverTheLastSeconds() {
        val speed = TransferProgress.Speed(windowMs = 4_000)
        assertEquals("one reading says nothing", 0, speed.add(0, 1_000))
        assertEquals("too soon to tell", 0, speed.add(200, 1_500))
        assertEquals(2_000, speed.add(2_000, 5_000))
        // Readings older than the window go: only the last 4 s count.
        assertEquals(1_000, speed.add(6_000, 9_000))
    }

    @Test
    fun aResumedAttemptStartsTheSpeedOver() {
        val speed = TransferProgress.Speed()
        speed.add(0, 0)
        speed.add(1_000, 800)
        assertEquals("not negative", 0, speed.add(2_000, 300))
        assertEquals(400, speed.add(3_000, 700))
    }

    @Test
    fun countersOfAnotherTransferDoNotCount() {
        assertEquals(40, TransferProgress.forFile(400, 1000, 1000))
        assertEquals("the last file's counters", 0, TransferProgress.forFile(400, 500, 1000))
        assertEquals(0, TransferProgress.forFile(0, 0, 0))
    }
}
