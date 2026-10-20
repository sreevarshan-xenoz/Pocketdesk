package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AutoReconnectTest {
    /** A session refused for want of a permission waits for the owner (N5). */
    @Test
    fun `a refusal for a permission is not retried`() {
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: sound not allowed"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: input not allowed"))
    }

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
    fun `with no cached internet identity, the LAN failure decides`() {
        assertTrue(
            AutoReconnect.retryable(
                "LAN connect failed (Connection to streaming server timed out: deadline has elapsed); " +
                    "no cached WAN identity for 192.0.2.10 — pair/connect on LAN first.",
            ),
        )
        assertFalse(
            AutoReconnect.retryable(
                "LAN connect failed (certificate does not match the pin); " +
                    "no cached WAN identity for 192.0.2.10 — pair/connect on LAN first.",
            ),
        )
    }

    @Test
    fun `an end the desktop chose is not retried`() {
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: kicked by user"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: session refused"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: not paired"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: permissions changed"))
        assertFalse(AutoReconnect.retryable("stream closed by the desktop: session end"))
    }

    @Test
    fun `a transport error the bridge reports as a close is retried`() {
        assertTrue(AutoReconnect.retryable("stream closed by the desktop: timed out"))
        assertTrue(AutoReconnect.retryable("stream closed by the desktop: connection reset by peer"))
        assertTrue(AutoReconnect.retryable("stream closed by the desktop: "))
    }

    @Test
    fun `trust and pairing problems wait for the person`() {
        assertFalse(AutoReconnect.retryable("Connection failed: certificate does not match the pin"))
        assertFalse(AutoReconnect.retryable("pairing declined"))
    }

    @Test
    fun `a word that merely contains pin is not a pairing problem`() {
        assertTrue(AutoReconnect.retryable("LAN: timed out · WAN: ping timed out while mapping ports"))
        assertFalse(errorSuggestsRepair("LAN: timed out · WAN: ping timed out while mapping ports"))
        assertTrue(errorSuggestsRepair("wrong PIN"))
    }

    /** I1: one rule for why a sound session or a remote ended. */
    @Test
    fun `a session ends by the computer's words, a silent link, or no answer`() {
        assertEquals("stream closed by the desktop: kicked", AutoReconnect.endedBy("kicked", wasWaiting = false))
        assertEquals("stream closed by the desktop: kicked", AutoReconnect.endedBy("kicked", wasWaiting = true))
        assertEquals("stalled", AutoReconnect.endedBy(null, wasWaiting = false))
        assertNull("the owner never answered: theirs to decide again", AutoReconnect.endedBy(null, wasWaiting = true))
        assertFalse(AutoReconnect.retryable(AutoReconnect.endedBy("kicked by user", false)!!))
        assertTrue(AutoReconnect.retryable(AutoReconnect.endedBy(null, false)!!))
    }

    @Test
    fun `retries back off, then stop after ten minutes`() {
        assertEquals(2_000L, AutoReconnect.delayMs(attempt = 0, elapsedMs = 0))
        assertEquals(4_000L, AutoReconnect.delayMs(attempt = 1, elapsedMs = 2_000))
        assertEquals(30_000L, AutoReconnect.delayMs(attempt = 9, elapsedMs = 120_000))
        assertNull(AutoReconnect.delayMs(attempt = 30, elapsedMs = 10 * 60_000L))
    }
}
