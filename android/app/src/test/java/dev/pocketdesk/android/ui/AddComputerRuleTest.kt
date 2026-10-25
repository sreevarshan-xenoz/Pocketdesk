package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AddComputerRuleTest {
    @Test
    fun `a MAC typed is kept, by colons or dashes, its spaces trimmed`() {
        assertEquals("00:00:5E:00:53:01", AddComputerRule.macToKeep("00:00:5E:00:53:01"))
        assertEquals("00-00-5e-00-53-01", AddComputerRule.macToKeep(" 00-00-5e-00-53-01 "))
    }

    @Test
    fun `anything else is not a MAC to keep`() {
        assertNull(AddComputerRule.macToKeep(""))
        assertNull(AddComputerRule.macToKeep("00:00:5E:00:53"))
        assertNull(AddComputerRule.macToKeep("00:00:5E:00:53:0G"))
    }
}
