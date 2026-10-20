package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.ClearRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Test

class ClearRuleTest {
    private fun decide(
        settingOn: Boolean = true,
        ownComputer: Boolean = true,
        sent: Boolean = true,
        shown: Boolean = true,
        clearable: Boolean = true,
        call: Boolean = false,
        withinBudget: Boolean = true,
    ) = ClearRule.decide(settingOn, ownComputer, sent, shown, clearable, call, withinBudget)

    /** L1: a clear clears only what the phone sent its own computer, and may clear. */
    @Test
    fun `a clear clears only what it may`() {
        assertEquals(Decision.Clear, decide())
        assertEquals(Decision.Off, decide(settingOn = false))
        assertEquals(Decision.NotOwnComputer, decide(ownComputer = false))
        assertEquals(Decision.NotSent, decide(sent = false))
        assertEquals(Decision.Gone, decide(shown = false))
        assertEquals(Decision.NotClearable, decide(clearable = false))
        assertEquals(Decision.Call, decide(call = true))
        assertEquals(Decision.TooMany, decide(withinBudget = false))
    }

    /** L1: thirty a minute, and the next minute more. */
    @Test
    fun `clears are held to thirty a minute`() {
        val budget = NotificationActionRule.Budget(ClearRule.PER_MINUTE)
        repeat(ClearRule.PER_MINUTE) { assertEquals(true, budget.take(1_000L)) }
        assertEquals(false, budget.take(1_000L))
        assertEquals(true, budget.take(61_000L))
    }
}
