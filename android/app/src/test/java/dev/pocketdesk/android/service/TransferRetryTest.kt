package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
    fun aDownloadAskedByTheInternetRoadIsTriedAgainOnceHomeIsBack() {
        // R6i's device run: after the Wi-Fi came back, the request went by a
        // link the internet road had brought back first.
        assertEquals(
            5L,
            TransferRetry.delayBefore(1, "The computer refused the download: downloads need the local network connection", enabled = true),
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
    fun aFolderSendStopsOnceThreeFilesInARowCouldNotReachTheComputer() {
        val streak = TransferRetry.FolderStreak()
        val gone = "The transfer failed: connection refused by the network"
        assertTrue(streak.record(sent = false, error = gone))
        assertTrue("one that went through starts the count again", streak.record(sent = true, error = null))
        assertTrue(streak.record(sent = false, error = gone))
        assertTrue(streak.record(sent = false, error = gone))
        assertFalse("the third in a row stops the send", streak.record(sent = false, error = gone))
    }

    @Test
    fun filesTheComputerRefusesOneByOneDoNotStopAFolder() {
        val streak = TransferRetry.FolderStreak()
        repeat(10) {
            assertTrue(streak.record(sent = false, error = "big.iso is larger than 8 GB"))
            assertTrue(streak.record(sent = false, error = "cannot open locked.db"))
        }
    }

    @Test
    fun whatThePhoneItselfRefusesIsNotRetriedInAnyLanguage() {
        for (refusal in listOf(
            TransferRetry.Refusal("No hay espacio para foto.jpg"),
            TransferRetry.Refusal("unsafe place in a folder: ../x"),
            TransferRetry.Refusal("cannot create a.jpg"),
        )) {
            assertNull(refusal.message, TransferRetry.delayAfter(1, refusal, enabled = true))
        }
        assertEquals(5L, TransferRetry.delayAfter(1, java.io.IOException("connection reset"), enabled = true))
        assertEquals(5L, TransferRetry.delayAfter(1, null, enabled = true))
    }

    @Test
    fun nothingIsRetriedWhenRetriesAreOff() {
        assertNull(TransferRetry.delayBefore(1, "reset", enabled = false))
    }

    @Test
    fun aFailureWithNoNetworkWaitsForOneWithoutUsingARetry() {
        val reset = java.io.IOException("Network is unreachable (os error 101)")
        // R6i: the A71's Wi-Fi took about 60 s to rejoin, and all three
        // retries (5, 15, 45 s) met no network.
        assertEquals(
            TransferRetry.Next.WhenOnline(180),
            TransferRetry.next(1, reset, enabled = true, hasNetwork = false, waitedOfflineSeconds = 0),
        )
        assertEquals(
            "what is left of the three minutes",
            TransferRetry.Next.WhenOnline(60),
            TransferRetry.next(3, reset, enabled = true, hasNetwork = false, waitedOfflineSeconds = 120),
        )
        assertEquals(
            "with the network back it is retry number one again, not a used one",
            TransferRetry.Next.After(5),
            TransferRetry.next(1, reset, enabled = true, hasNetwork = true, waitedOfflineSeconds = 90),
        )
    }

    @Test
    fun aPhoneWhoseNetworkNeverComesBackGivesUpAndSaysSo() {
        val reset = java.io.IOException("Network is unreachable (os error 101)")
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = true),
            TransferRetry.next(2, reset, enabled = true, hasNetwork = false, waitedOfflineSeconds = 180),
        )
    }

    @Test
    fun aRefusalIsFinalEvenWithNoNetworkAndOffMeansOff() {
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = false),
            TransferRetry.next(
                1,
                TransferRetry.Refusal("No room for photo.jpg"),
                enabled = true,
                hasNetwork = false,
                waitedOfflineSeconds = 0,
            ),
        )
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = false),
            TransferRetry.next(1, java.io.IOException("reset"), enabled = false, hasNetwork = false, waitedOfflineSeconds = 0),
        )
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = false),
            TransferRetry.next(4, java.io.IOException("reset"), enabled = true, hasNetwork = true, waitedOfflineSeconds = 0),
        )
    }

    @Test
    fun anyCarrierNetworkWillDoButAVpnAloneOrASuspendedOneWillNot() {
        fun seen(vpn: Boolean = false, carrier: Boolean = true, suspended: Boolean = false) =
            TransferRetry.SeenNetwork(vpn, carrier, suspended)
        // Mobile data alone: a computer at home is reached by the internet road (2c).
        assertTrue(TransferRetry.hasNetwork(listOf(seen()), sharing = false))
        assertFalse("nothing at all", TransferRetry.hasNetwork(emptyList(), sharing = false))
        assertFalse("a VPN left over from a lost network", TransferRetry.hasNetwork(listOf(seen(vpn = true)), sharing = false))
        assertFalse("mobile data during a call", TransferRetry.hasNetwork(listOf(seen(suspended = true)), sharing = false))
        assertFalse("a network with no carrier", TransferRetry.hasNetwork(listOf(seen(carrier = false)), sharing = false))
        assertTrue("the Wi-Fi under a VPN", TransferRetry.hasNetwork(listOf(seen(vpn = true), seen()), sharing = false))
        assertTrue("this phone's hotspot, a computer on it", TransferRetry.hasNetwork(emptyList(), sharing = true))
    }

    @Test
    fun aJobsSecondFileDoesNotWaitAgainOnceTheBudgetIsSpent() {
        val reset = java.io.IOException("Network is unreachable (os error 101)")
        // A folder's first file waited the 3 minutes; the next gives up at once.
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = true),
            TransferRetry.next(1, reset, enabled = true, hasNetwork = false, waitedOfflineSeconds = 181),
        )
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
