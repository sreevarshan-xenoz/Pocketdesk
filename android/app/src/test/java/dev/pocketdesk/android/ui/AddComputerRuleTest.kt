package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AddComputerRuleTest {
    @Test
    fun `a MAC typed is kept with auto-connect off, as on`() {
        // AV3: it was kept only with "Connect to it when the app opens" on.
        assertEquals("00:00:5E:00:53:01", AddComputerRule.macToKeep("00:00:5E:00:53:01", connectsOnOpen = false))
        assertEquals("00:00:5E:00:53:01", AddComputerRule.macToKeep("00:00:5E:00:53:01", connectsOnOpen = true))
    }

    @Test
    fun `by colons or dashes, its spaces trimmed`() {
        assertEquals("00-00-5e-00-53-01", AddComputerRule.macToKeep(" 00-00-5e-00-53-01 ", connectsOnOpen = false))
    }

    @Test
    fun `anything else is not a MAC to keep`() {
        assertNull(AddComputerRule.macToKeep("", connectsOnOpen = true))
        assertNull(AddComputerRule.macToKeep("00:00:5E:00:53", connectsOnOpen = true))
        assertNull(AddComputerRule.macToKeep("00:00:5E:00:53:0G", connectsOnOpen = false))
    }
}
