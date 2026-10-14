package dev.pocketdesk.android.ui

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
}
