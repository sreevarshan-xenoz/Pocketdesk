package dev.pocketdesk.android.automation

import dev.pocketdesk.android.automation.CommandRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CommandRuleTest {
    /** L6: a command goes only from its own computer, with the setting on, within the budget. */
    @Test
    fun `a command goes only when it may`() {
        assertEquals(Decision.Send, CommandRule.decide(true, true, true))
        assertEquals(Decision.Off, CommandRule.decide(false, true, true))
        assertEquals(Decision.NotOwnComputer, CommandRule.decide(true, false, true))
        assertEquals(Decision.TooMany, CommandRule.decide(true, true, false))
    }

    /** L6: "Only to" is the app chosen while installed, else the first known one. */
    @Test
    fun `only to is a known app installed`() {
        val tasker = "net.dinglisch.android.taskerm"
        val automate = "com.llamalab.automate"
        assertEquals(automate, CommandRule.onlyTo(automate, setOf(tasker, automate)))
        assertEquals(tasker, CommandRule.onlyTo(null, setOf(automate, tasker)))
        assertEquals("the chosen one gone", tasker, CommandRule.onlyTo(automate, setOf(tasker)))
        assertNull(CommandRule.onlyTo(null, emptySet()))
    }
}
