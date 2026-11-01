package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.PhoneActionRule.Action
import dev.pocketdesk.android.control.PhoneActionRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class AppOpenRuleTest {
    private fun decide(
        action: Action = Action.OPEN,
        own: Boolean = true,
        locked: Boolean = false,
        awake: Boolean = true,
        appsOn: Boolean = true,
        listed: Boolean = true,
        service: Boolean = true,
    ) = PhoneActionRule.decide(
        action = action,
        on = true,
        own = own,
        service = service,
        sdk = 34,
        locked = locked,
        awake = awake,
        screenshots = true,
        withinBudget = true,
        appsOn = appsOn,
        listed = listed,
    )

    @Test
    fun `with the switch off, the list and the open are refused and the computer is told off`() {
        for (action in listOf(Action.APPS, Action.OPEN)) {
            val refused = decide(action = action, appsOn = false)
            assertEquals(Decision.OFF, refused)
            assertEquals(AppOpenRule.OFF, AppOpenRule.refusal(refused))
        }
    }

    @Test
    fun `a computer that is not the phone's own is refused and never told`() {
        for (action in listOf(Action.APPS, Action.OPEN)) {
            val refused = decide(action = action, own = false)
            assertEquals(Decision.NOT_OWN, refused)
            assertNull(AppOpenRule.refusal(refused))
        }
        // The switch off and not its own: the decision is OFF, yet nothing is said to it.
        val off = decide(own = false, appsOn = false)
        assertEquals(Decision.OFF, off)
        assertNull(AppOpenRule.tell(off, own = false))
        assertEquals(AppOpenRule.OFF, AppOpenRule.tell(off, own = true))
        assertEquals(AppOpenRule.LOCKED, AppOpenRule.tell(Decision.LOCKED, own = true))
        assertNull(AppOpenRule.tell(Decision.LOCKED, own = false))
    }

    @Test
    fun `a locked phone says locked, and a sleeping one asleep, for an open`() {
        assertEquals(AppOpenRule.LOCKED, AppOpenRule.refusal(decide(locked = true)))
        assertEquals(AppOpenRule.ASLEEP, AppOpenRule.refusal(decide(awake = false)))
        assertEquals(AppOpenRule.LOCKED, AppOpenRule.refusal(decide(action = Action.APPS, locked = true)))
    }

    @Test
    fun `an app that was not listed is refused`() {
        assertEquals(AppOpenRule.NOT_LISTED, AppOpenRule.refusal(decide(listed = false)))
    }

    @Test
    fun `an allowed ask has no refusal`() {
        assertNull(AppOpenRule.refusal(Decision.DO))
        assertEquals(Decision.DO, decide())
        assertEquals(AppOpenRule.TOO_MANY, AppOpenRule.refusal(Decision.TOO_MANY))
    }

    @Test
    fun `without the screen asked for, an open is as before`() {
        for (service in listOf(true, false)) {
            for (casting in listOf(true, false)) {
                assertEquals(AppOpenRule.Plan.TAP, AppOpenRule.plan(false, service, casting))
            }
        }
    }

    @Test
    fun `with the screen asked for, the service is needed and the cast is reused`() {
        assertEquals(AppOpenRule.Plan.CONTROL_OFF, AppOpenRule.plan(true, service = false, casting = true))
        assertEquals(AppOpenRule.Plan.CONTROL_OFF, AppOpenRule.plan(true, service = false, casting = false))
        // A cast already shown to the computer: start the app, no new consent.
        assertEquals(AppOpenRule.Plan.START, AppOpenRule.plan(true, service = true, casting = true))
        // None yet: the owner is asked once, then the app opens.
        assertEquals(AppOpenRule.Plan.SHOW_SCREEN_FIRST, AppOpenRule.plan(true, service = true, casting = false))
    }

    @Test
    fun `the report names the ask, the app for an open, and the result`() {
        val open = AppOpenRule.report("open", "org.example.app", AppOpenRule.LOCKED)
        assertEquals("open", open.getString("about"))
        assertEquals("org.example.app", open.getString("app"))
        assertEquals("locked", open.getString("result"))
        val list = AppOpenRule.report("apps", null, AppOpenRule.OFF)
        assertFalse(list.has("app"))
        assertEquals("off", list.getString("result"))
    }

    @Test
    fun `the words are the computer's`() {
        // core::protocol::phone_action::AppResult::as_str.
        val words = listOf(
            AppOpenRule.OPENED, AppOpenRule.NEEDS_SCREEN, AppOpenRule.CONTROL_OFF, AppOpenRule.LOCKED,
            AppOpenRule.ASLEEP, AppOpenRule.OFF, AppOpenRule.NOT_LISTED, AppOpenRule.NOT_FOUND,
            AppOpenRule.TOO_MANY, AppOpenRule.FAILED,
        )
        assertEquals(
            listOf("opened", "needs_screen", "control_off", "locked", "asleep", "off", "not_listed", "not_found", "too_many", "failed"),
            words,
        )
    }
}
