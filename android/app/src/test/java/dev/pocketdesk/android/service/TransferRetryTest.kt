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
    fun aHomeNetworkAlwaysCountsAndMobileDataOnlyForAComputerReachedFromAway() {
        fun seen(
            vpn: Boolean = false,
            local: Boolean = false,
            cellular: Boolean = false,
            suspended: Boolean = false,
            internet: Boolean = true,
        ) = TransferRetry.SeenNetwork(vpn, local, cellular, suspended, internet)
        val wifi = seen(local = true)
        val data = seen(cellular = true)
        assertTrue(TransferRetry.hasNetwork(listOf(wifi), sharing = false, fromAway = false))
        assertTrue("Wi-Fi with no internet still reaches home", TransferRetry.hasNetwork(listOf(seen(local = true, internet = false)), false, false))
        assertTrue("mobile data, by the internet road", TransferRetry.hasNetwork(listOf(data), false, fromAway = true))
        assertFalse("mobile data cannot reach a computer only at home", TransferRetry.hasNetwork(listOf(data), false, fromAway = false))
        assertFalse("a network for calls only (IMS)", TransferRetry.hasNetwork(listOf(seen(cellular = true, internet = false)), false, true))
        assertFalse("mobile data during a call", TransferRetry.hasNetwork(listOf(seen(cellular = true, suspended = true)), false, true))
        assertFalse("nothing at all", TransferRetry.hasNetwork(emptyList(), false, true))
        assertFalse("a VPN left over from a lost network", TransferRetry.hasNetwork(listOf(seen(vpn = true, local = true)), false, true))
        assertTrue("the Wi-Fi under a VPN", TransferRetry.hasNetwork(listOf(seen(vpn = true), wifi), false, false))
        assertTrue("this phone's hotspot, a computer on it", TransferRetry.hasNetwork(emptyList(), sharing = true, fromAway = false))
    }

    @Test
    fun aComputerIsReachedFromAwayByItsIdentityOrAnAddressThatIsNotAHomeOne() {
        for (home in listOf("192.168.1.20", "10.0.0.5", "172.16.4.2", "169.254.3.3", "desk.local")) {
            assertFalse(home, TransferRetry.reachedFromAway(home, wanIdentity = false))
            assertTrue(home, TransferRetry.reachedFromAway(home, wanIdentity = true))
        }
        for (away in listOf("100.101.102.103", "203.0.113.7", "desk.example.org", "2001:db8::1")) {
            assertTrue(away, TransferRetry.reachedFromAway(away, wanIdentity = false))
        }
    }

    @Test
    fun theWaitIsOnePerOutageAcrossJobsAndAnEndedOutageWaitsAfresh() {
        val budget = TransferRetry.OfflineBudget(forgetAfterMs = 600_000)
        assertEquals(0, budget.waited(0))
        // A folder's first download waited the 3 minutes; the next one, a
        // job of its own in the same outage, does not wait again.
        budget.add(180, 180_000)
        assertEquals(180, budget.waited(181_000))
        assertEquals(
            TransferRetry.Next.Stop(noNetwork = true),
            TransferRetry.next(1, java.io.IOException("unreachable"), true, hasNetwork = false, waitedOfflineSeconds = budget.waited(181_000)),
        )
        // Ten minutes later a new transfer waits again.
        assertEquals(0, budget.waited(180_000 + 600_000))
        // A success ends the outage too.
        budget.add(60, 900_000)
        budget.reset()
        assertEquals(0, budget.waited(900_001))
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
