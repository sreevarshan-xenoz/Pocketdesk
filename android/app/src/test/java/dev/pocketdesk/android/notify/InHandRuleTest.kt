package dev.pocketdesk.android.notify

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class InHandRuleTest {
    @Test
    fun `marked with the switch on, the screen on and the phone unlocked`() {
        assertTrue(InHandRule.marks(on = true, interactive = true, locked = false))
    }

    @Test
    fun `not with the switch off, the screen off, or the phone locked`() {
        assertFalse(InHandRule.marks(on = false, interactive = true, locked = false))
        assertFalse(InHandRule.marks(on = true, interactive = false, locked = false))
        assertFalse(InHandRule.marks(on = true, interactive = true, locked = true))
    }
}
