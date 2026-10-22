package dev.pocketdesk.android.discovery

import dev.pocketdesk.android.HostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FoundComputersTest {
    @Test
    fun lanAddressesAreTriedBeforeTailscaleAndIpv6() {
        val all = listOf("fe80::1", "100.101.102.103", "192.168.1.20", "172.17.0.1")
        assertEquals(
            listOf("192.168.1.20", "172.17.0.1", "100.101.102.103", "fe80::1"),
            FoundComputers.rankAddresses(all),
        )
        assertEquals(
            "a 172 outside 16-31 is not private, but still IPv4",
            listOf("172.40.1.1", "fe80::2"),
            FoundComputers.rankAddresses(listOf("fe80::2", "172.40.1.1")),
        )
        assertEquals(
            "at most four, duplicates once",
            listOf("10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"),
            FoundComputers.rankAddresses(
                listOf("10.0.0.1", "10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"),
            ),
        )
        assertEquals(emptyList<String>(), FoundComputers.rankAddresses(emptyList()))
    }

    @Test
    fun aRecordFillsBothPortsAndBadPortsFallBack() {
        val ok = FoundComputers.fromRecord(
            "DESK",
            1800,
            mapOf("name" to "Desk", "id" to "abc", "stream" to "4800"),
            listOf("192.168.1.20"),
        )!!
        assertEquals(FoundComputer("DESK", "Desk", "abc", "192.168.1.20", 1800, 4800), ok)
        assertEquals(listOf("192.168.1.20"), ok.candidates)

        val bad = FoundComputers.fromRecord("DESK", 0, mapOf("stream" to "99999"), listOf("10.0.0.2"))!!
        assertEquals(HostStore.DEFAULT_CONTROL_PORT, bad.controlPort)
        assertEquals(HostStore.DEFAULT_STREAMING_PORT, bad.streamPort)
        assertEquals("the instance name stands in for a missing one", "DESK", bad.name)
        assertEquals("", bad.id)

        assertNull(FoundComputers.fromRecord("DESK", 1716, emptyMap(), emptyList()))
    }

    @Test
    fun aNameIsCleanedBeforeItIsShown() {
        val found = FoundComputers.fromRecord(
            "x",
            1716,
            mapOf("name" to "Desk\u202Egnp.exe"),
            listOf("10.0.0.2"),
        )!!
        assertEquals("Deskgnp.exe", found.name)
    }

    @Test
    fun aSavedComputerIsRecognisedByItsAddressOnly() {
        val saved = listOf(
            HostStore.Host("192.168.1.20", 4716, 1716),
            HostStore.Host("192.168.1.50", 4716, 1716),
        )
        val atSaved = computer("a", "192.168.1.20", id = "desk-a")
        // The computer saved at .50 now answers at .77: offer the new
        // address rather than mark the old card online.
        val moved = computer("b", "192.168.1.77", id = "desk-b")
        val view = FoundComputers.merge(listOf(atSaved, moved), saved)
        assertEquals(listOf(moved), view.unsaved)
        assertEquals(setOf("192.168.1.20"), view.savedOnline)
    }

    @Test
    fun oneComputerAnnouncedTwiceIsListedOnce() {
        val view = FoundComputers.merge(
            listOf(computer("a", "10.0.0.2"), computer("a (2)", "10.0.0.2")),
            emptyList(),
        )
        assertEquals(1, view.unsaved.size)
    }

    /** O3: a saved computer's id announced at an address no card holds is a candidate, no more. */
    @Test
    fun aSavedComputerAtANewAddressIsACandidateToMove() {
        val saved = listOf(
            HostStore.Host("192.168.1.20", 4716),
            HostStore.Host("mypc.local", 4716),
            HostStore.Host("192.168.1.40", 4716),
            HostStore.Host("192.168.1.41", 4716),
        )
        val ids = mapOf("192.168.1.20" to "desk", "mypc.local" to "named", "192.168.1.40" to "twin", "192.168.1.41" to "twin")
        val found = listOf(
            computer("Desk", "192.168.1.21", id = "desk"),
            // Still at its saved address: not moved.
            computer("Desk", "192.168.1.20", id = "desk"),
            // Saved by name: it keeps its name.
            computer("Named", "192.168.1.30", id = "named"),
            // Two cards for one id: which one is not guessed.
            computer("Twin", "192.168.1.42", id = "twin"),
            // No id, or one no card was paired with.
            computer("Other", "192.168.1.50"),
            computer("Stranger", "192.168.1.51", id = "stranger"),
        )
        val moved = FoundComputers.moved(found, saved) { ids[it] }
        assertEquals(listOf("192.168.1.20" to "192.168.1.21"), moved.map { it.saved.address to it.found.address })
    }

    /** O3 review: only a private address is a new home, and a few tries a card, so a spoof cannot block the real one. */
    @Test
    fun aMoveIsTriedAtAFewPrivateAddresses() {
        val saved = listOf(HostStore.Host("192.168.1.20", 4716))
        val found = listOf(
            computer("Desk", "8.8.8.8", id = "desk"),
            computer("Desk", "127.0.0.1", id = "desk"),
            computer("Desk", "169.254.1.2", id = "desk"),
            computer("Desk", "10.0.0.1", id = "desk"),
            computer("Desk", "10.0.0.2", id = "desk"),
            computer("Desk", "10.0.0.2", id = "desk"),
            computer("Desk", "10.0.0.3", id = "desk"),
            computer("Desk", "10.0.0.4", id = "desk"),
        )
        val moved = FoundComputers.moved(found, saved) { "desk" }
        assertEquals(listOf("10.0.0.1", "10.0.0.2", "10.0.0.3"), moved.map { it.found.address })
    }

    private fun computer(service: String, address: String, id: String = "") =
        FoundComputer(service, service, id, address, 1716, 4716)
}
