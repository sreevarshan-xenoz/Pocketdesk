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
        appsOn: Boolean = true,
        listed: Boolean = true,
    ) = PhoneActionRule.decide(action, on, own, service, sdk, locked, awake, screenshots, withinBudget, appsOn, listed)

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

    @Test
    fun `apps are listed with their own switch, no service needed`() {
        assertEquals(Action.APPS, Action.parse("apps"))
        assertEquals(Decision.DO, decide(action = Action.APPS, on = false, service = false, awake = false, sdk = 21))
        assertEquals(Decision.LOCKED, decide(action = Action.APPS, locked = true))
        assertEquals(Decision.OFF, decide(action = Action.APPS, appsOn = false))
        assertEquals(Decision.NOT_OWN, decide(action = Action.APPS, own = false))
        assertEquals(Decision.TOO_MANY, decide(action = Action.APPS, withinBudget = false))
    }

    @Test
    fun `an app is opened on an unlocked phone, awake, through the service`() {
        assertEquals(Action.OPEN, Action.parse("open"))
        assertEquals(Decision.DO, decide(action = Action.OPEN, on = false, screenshots = false))
        assertEquals(Decision.OFF, decide(action = Action.OPEN, appsOn = false))
        assertEquals("\"Tap to open\" then", Decision.DO, decide(action = Action.OPEN, service = false))
        assertEquals(Decision.NOT_LISTED, decide(action = Action.OPEN, listed = false))
        assertEquals(Decision.LOCKED, decide(action = Action.OPEN, locked = true))
        assertEquals(Decision.ASLEEP, decide(action = Action.OPEN, awake = false))
    }
}
