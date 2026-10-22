package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.PhoneActionRule.Action
import dev.pocketdesk.android.control.PhoneActionRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PhoneActionRuleTest {
    private fun decide(
        action: Action = Action.SCREENSHOT,
        on: Boolean = true,
        own: Boolean = true,
        service: Boolean = true,
        sdk: Int = 34,
        locked: Boolean = false,
        awake: Boolean = true,
        screenshots: Boolean = true,
        withinBudget: Boolean = true,
    ) = PhoneActionRule.decide(action, on, own, service, sdk, locked, awake, screenshots, withinBudget)

    @Test
    fun `an action reads back from its word, and nothing else does`() {
        assertEquals(Action.SCREENSHOT, Action.parse("screenshot"))
        assertEquals(Action.LOCK, Action.parse("lock"))
        assertNull(Action.parse("unlock"))
        assertNull(Action.parse(null))
    }

    @Test
    fun `done only with the switch, from the own computer, with the service`() {
        assertEquals(Decision.DO, decide())
        assertEquals(Decision.OFF, decide(on = false, own = false))
        assertEquals(Decision.NOT_OWN, decide(own = false))
        assertEquals(Decision.TOO_OLD, decide(sdk = 27))
        assertEquals(Decision.NO_SERVICE, decide(service = false))
        assertEquals(Decision.TOO_MANY, decide(withinBudget = false))
    }

    @Test
    fun `a screenshot needs an unlocked phone and screenshots sent, a lock neither`() {
        assertEquals(Decision.LOCKED, decide(locked = true))
        assertEquals(Decision.ASLEEP, decide(awake = false))
        assertEquals(Decision.SCREENSHOTS_OFF, decide(screenshots = false))
        assertEquals(Decision.DO, decide(action = Action.LOCK, locked = true, awake = false, screenshots = false))
    }
}
