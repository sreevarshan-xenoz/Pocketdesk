package dev.pocketdesk.android.notify

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class InHandRuleTest {
    @Test
    fun `marked with the switch on, the screen on and the phone unlocked`() {
        assertTrue(InHandRule.marks(on = true, interactive = true, locked = false, remote = false))
    }

    @Test
    fun `not with the switch off, the screen off, or the phone locked`() {
        assertFalse(InHandRule.marks(on = false, interactive = true, locked = false, remote = false))
        assertFalse(InHandRule.marks(on = true, interactive = false, locked = false, remote = false))
        assertFalse(InHandRule.marks(on = true, interactive = true, locked = true, remote = false))
    }

    @Test
    fun `not while Pocketdesk's Remote is in front, whose owner looks at the computer`() {
        assertFalse(InHandRule.marks(on = true, interactive = true, locked = false, remote = true))
    }
}
