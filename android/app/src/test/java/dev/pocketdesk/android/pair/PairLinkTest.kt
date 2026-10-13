package dev.pocketdesk.android.pair

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PairLinkTest {
    /** The exact URI core/src/local/pair_link.rs encodes; both tests pin it. */
    private val fixture =
        "pocketdesk://pair?v=1&name=Desk%20%26%20Co&id=d-1&a=192.168.1.5,100.80.1.2" +
            "&cp=1716&sp=4716&pin=012345" +
            "&fp=abababababababababababababababababababababababababababababababab" +
            "&ep=eyJlbmRwb2ludElkIjoieCIsInJlbGF5VXJscyI6W10sImRpcmVjdEFkZHJzIjpbXX0"

    @Test
    fun parsesTheDesktopsEncoding() {
        val link = PairLink.parse(fixture).getOrThrow()
        assertEquals("Desk & Co", link.name)
        assertEquals("d-1", link.deviceId)
        assertEquals(listOf("192.168.1.5", "100.80.1.2"), link.addresses)
        assertEquals(1716, link.controlPort)
        assertEquals(4716, link.streamPort)
        assertEquals("012345", link.pin)
        assertEquals("ab".repeat(32), link.fingerprint)
        assertEquals("""{"endpointId":"x","relayUrls":[],"directAddrs":[]}""", link.endpoint)
    }

    @Test
    fun optionalPartsMayBeAbsentAndPortsDefault() {
        val link = PairLink.parse("pocketdesk://pair?v=1&a=10.0.0.2&pin=123456").getOrThrow()
        assertNull(link.fingerprint)
        assertNull(link.endpoint)
        assertEquals(1716, link.controlPort)
        assertEquals(4716, link.streamPort)
    }

    @Test
    fun malformedLinksAreRejected() {
        for (bad in listOf(
            "https://example.com",
            "pocketdesk://pair?v=2&a=1.2.3.4&pin=123456",
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=12",
            "pocketdesk://pair?v=1&pin=123456",
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&cp=x",
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&ep=%%%",
            // A fingerprint must be exactly 64 hex digits.
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&fp=" + "zz".repeat(32),
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&fp=abcd",
            "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&fp=" + "a".repeat(61) + "%C3%A9b",
        )) {
            assertTrue(bad, PairLink.parse(bad).isFailure)
        }
    }
}
