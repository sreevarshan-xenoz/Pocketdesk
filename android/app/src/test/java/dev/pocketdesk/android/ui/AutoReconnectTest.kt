package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AutoReconnectTest {
    @Test
    fun `a link that went silent is retried`() {
        assertTrue(AutoReconnect.retryable("video feed stalled — connection closed"))
    }

    @Test
    fun `a desktop that cannot be reached yet is retried`() {
        assertTrue(AutoReconnect.retryable("LAN: Connection failed: timed out · WAN: no route"))
        assertTrue(AutoReconnect.retryable("Connection failed: Connection refused (os error 111)"))
        assertTrue(AutoReconnect.retryable("Network is unreachable"))
    }

    @Test
    fun `an end the desktop chose is not retried`() {
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: kicked by user"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: session refused"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: not paired"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: permissions changed"))
    }

    @Test
    fun `trust and pairing problems wait for the person`() {
        assertFalse(AutoReconnect.retryable("Connection failed: certificate does not match the pin"))
        assertFalse(AutoReconnect.retryable("pairing declined"))
        assertFalse(
            AutoReconnect.retryable(
                "LAN connect failed (timed out); no cached WAN identity for 192.0.2.10 — pair/connect on LAN first.",
            ),
        )
    }

    @Test
    fun `retries back off, then stop after ten minutes`() {
        assertEquals(2_000L, AutoReconnect.delayMs(attempt = 0, elapsedMs = 0))
        assertEquals(4_000L, AutoReconnect.delayMs(attempt = 1, elapsedMs = 2_000))
        assertEquals(30_000L, AutoReconnect.delayMs(attempt = 9, elapsedMs = 120_000))
        assertNull(AutoReconnect.delayMs(attempt = 30, elapsedMs = 10 * 60_000L))
    }
}
