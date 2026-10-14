package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ControlLinkTest {
    @Test
    fun `a session with no control connection connects`() {
        assertTrue(ControlLink.needsConnect(live = null, address = "192.0.2.10", port = 1716))
    }

    @Test
    fun `a session already connected to its computer leaves it alone`() {
        assertFalse(ControlLink.needsConnect(live = "192.0.2.10" to 1716, address = "192.0.2.10", port = 1716))
    }

    @Test
    fun `a connection to another computer is replaced by the session's`() {
        assertTrue(ControlLink.needsConnect(live = "192.0.2.20" to 1716, address = "192.0.2.10", port = 1716))
        assertTrue(ControlLink.needsConnect(live = "192.0.2.10" to 1717, address = "192.0.2.10", port = 1716))
    }

    @Test
    fun `failed dials back off to a minute, and a success starts over`() {
        assertEquals(4_000L, ControlLink.nextWaitMs(failures = 0))
        assertEquals(8_000L, ControlLink.nextWaitMs(failures = 1))
        assertEquals(32_000L, ControlLink.nextWaitMs(failures = 3))
        assertEquals(60_000L, ControlLink.nextWaitMs(failures = 10))
    }
}
