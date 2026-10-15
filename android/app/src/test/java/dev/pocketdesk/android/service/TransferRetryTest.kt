package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferRetryTest {
    @Test
    fun aBrokenTransferIsTriedThreeMoreTimesFurtherApart() {
        val error = "The transfer failed: connection reset"
        assertEquals(5L, TransferRetry.delayBefore(1, error, enabled = true))
        assertEquals(15L, TransferRetry.delayBefore(2, error, enabled = true))
        assertEquals(45L, TransferRetry.delayBefore(3, error, enabled = true))
        assertNull("then it stops", TransferRetry.delayBefore(4, error, enabled = true))
    }

    @Test
    fun aRefusalOrAFileThatCannotBeSentIsNotRetried() {
        for (error in listOf(
            "The computer refused the file: the phone may not send files",
            "photo.jpg is empty",
            "cannot open photo.jpg",
        )) {
            assertNull(error, TransferRetry.delayBefore(1, error, enabled = true))
        }
    }

    @Test
    fun aRefusedDownloadIsFinalButABusyComputerIsWorthWaitingFor() {
        assertNull(TransferRetry.delayBefore(1, "The computer refused the download: the offer expired", enabled = true))
        assertNull(TransferRetry.delayBefore(1, "a size of 12 bytes was not accepted", enabled = true))
        assertEquals(
            5L,
            TransferRetry.delayBefore(1, "The computer refused the file: the desktop is busy with other transfers", enabled = true),
        )
        assertEquals(
            5L,
            TransferRetry.delayBefore(1, "The computer refused the file: this phone's last transfer is still running", enabled = true),
        )
    }

    @Test
    fun aComputerThatIsNotListeningIsTriedAgain() {
        // The network's "connection refused" is not the computer saying no.
        for (error in listOf(
            "Connection refused (os error 111)",
            "No connection could be made because the target machine actively refused it. (os error 10061)",
        )) {
            assertEquals(error, 5L, TransferRetry.delayBefore(1, error, enabled = true))
        }
    }

    @Test
    fun nothingIsRetriedWhenRetriesAreOff() {
        assertNull(TransferRetry.delayBefore(1, "reset", enabled = false))
    }

    @Test
    fun aSourceIdIsStableAndChangesWithTheFile() {
        val id = SourceId.of("content://media/external/video/1", 300, 1_700_000_000_000)
        assertEquals(32, id.length)
        assertTrue(id.all { it in '0'..'9' || it in 'a'..'f' })
        assertEquals(id, SourceId.of("content://media/external/video/1", 300, 1_700_000_000_000))
        assertNotEquals(id, SourceId.of("content://media/external/video/1", 301, 1_700_000_000_000))
        assertNotEquals(id, SourceId.of("content://media/external/video/1", 300, 1_700_000_000_001))
        assertNotEquals(id, SourceId.of("content://media/external/video/2", 300, 1_700_000_000_000))
    }
}
