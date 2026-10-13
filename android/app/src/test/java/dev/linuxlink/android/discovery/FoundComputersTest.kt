package dev.linuxlink.android.discovery

import dev.linuxlink.android.HostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FoundComputersTest {
    @Test
    fun theLanAddressWinsOverTailscaleAndIpv6() {
        val all = listOf("fe80::1", "100.101.102.103", "192.168.1.20")
        assertEquals("192.168.1.20", FoundComputers.pickAddress(all))
        assertEquals("10.0.0.5", FoundComputers.pickAddress(listOf("100.64.0.1", "10.0.0.5")))
        assertEquals("172.20.1.1", FoundComputers.pickAddress(listOf("172.20.1.1")))
        assertEquals(
            "a 172 outside 16-31 is not private, but still IPv4",
            "172.40.1.1",
            FoundComputers.pickAddress(listOf("fe80::2", "172.40.1.1")),
        )
        assertEquals("fe80::1", FoundComputers.pickAddress(listOf("fe80::1")))
        assertNull(FoundComputers.pickAddress(emptyList()))
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
            mapOf("name" to "Desk‮gnp.exe"),
            listOf("10.0.0.2"),
        )!!
        assertEquals("Deskgnp.exe", found.name)
    }

    @Test
    fun savedComputersAreRecognisedByAddressOrByTheirPairedId() {
        val sameAddress = computer("a", "192.168.1.20", id = "")
        val moved = computer("b", "192.168.1.77", id = "desk-b")
        val stranger = computer("c", "192.168.1.30", id = "desk-c")
        val saved = listOf(
            HostStore.Host("192.168.1.20", 4716, 1716),
            HostStore.Host("192.168.1.50", 4716, 1716),
        )
        val view = FoundComputers.merge(listOf(sameAddress, moved, stranger), saved) { address ->
            if (address == "192.168.1.50") "desk-b" else null
        }
        assertEquals(listOf(stranger), view.unsaved)
        assertEquals(setOf("192.168.1.20", "192.168.1.50"), view.savedOnline)
    }

    @Test
    fun oneComputerAnnouncedTwiceIsListedOnce() {
        val view = FoundComputers.merge(
            listOf(computer("a", "10.0.0.2"), computer("a (2)", "10.0.0.2")),
            emptyList(),
        ) { null }
        assertEquals(1, view.unsaved.size)
    }

    private fun computer(service: String, address: String, id: String = "") =
        FoundComputer(service, service, id, address, 1716, 4716)
}
