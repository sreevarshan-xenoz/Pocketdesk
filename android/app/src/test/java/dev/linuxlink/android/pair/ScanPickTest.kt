package dev.linuxlink.android.pair

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ScanPickTest {
    @Test
    fun theFirstPocketdeskCodeInViewIsTaken() {
        val seen = listOf(null, "https://example.com", "pocketdesk://pair?v=1&a=1.2.3.4&pin=123456", "pocketdesk://pair?v=1&x")
        assertEquals("pocketdesk://pair?v=1&a=1.2.3.4&pin=123456", firstPairingCode(seen))
    }

    @Test
    fun otherCodesAreIgnored() {
        assertNull(firstPairingCode(listOf("WIFI:S:home;;", "pocketdesk://other", null)))
        assertNull(firstPairingCode(emptyList()))
    }
}
