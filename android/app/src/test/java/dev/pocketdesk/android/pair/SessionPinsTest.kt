package dev.pocketdesk.android.pair

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionPinsTest {
    /**
     * BC3: a session to a found computer pins its key on first use, and the
     * computer is saved only once it pairs; one that never paired left the
     * pin with no card, so another computer at the address could not be
     * paired short of clearing the app's data.
     */
    @Test
    fun `a session's pin is forgotten when no card is saved there`() {
        val saved = listOf("192.0.2.1", "192.0.2.2")
        assertTrue(SessionPins.forgets("192.0.2.9", saved))
        assertFalse(SessionPins.forgets("192.0.2.2", saved))
        assertFalse("no address, nothing to forget", SessionPins.forgets("", saved))
    }
}
