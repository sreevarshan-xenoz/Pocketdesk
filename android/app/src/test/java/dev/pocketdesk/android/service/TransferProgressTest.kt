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
    fun countersOfAnotherTransferDoNotCount() {
        assertEquals(40, TransferProgress.forFile(400, 1000, 1000))
        assertEquals("the last file's counters", 0, TransferProgress.forFile(400, 500, 1000))
        assertEquals(0, TransferProgress.forFile(0, 0, 0))
    }
}
