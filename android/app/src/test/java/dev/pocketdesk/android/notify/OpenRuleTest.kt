package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenRuleTest {
    private fun decide(
        settingOn: Boolean = true,
        ownComputer: Boolean = true,
        locked: Boolean = false,
        shown: Boolean = true,
        service: Boolean = true,
        withinBudget: Boolean = true,
    ) = OpenRule.decide(settingOn, ownComputer, locked, shown, service, withinBudget)

    @Test
    fun anUnlockedPhoneOpensWithTheServiceElseOffers() {
        assertEquals(OpenRule.Decision.OPEN, decide())
        assertEquals(OpenRule.Decision.OFFER, decide(service = false))
    }

    @Test
    fun eachRefusalHasItsReason() {
        assertEquals(OpenRule.Decision.NOT_OWN_COMPUTER, decide(ownComputer = false, settingOn = false))
        assertEquals(OpenRule.Decision.OFF, decide(settingOn = false))
        assertEquals(OpenRule.Decision.GONE, decide(shown = false, locked = true))
        assertEquals(OpenRule.Decision.LOCKED, decide(locked = true))
        assertEquals(OpenRule.Decision.TOO_MANY, decide(withinBudget = false))
    }

    @Test
    fun theComputerHearsHowItWent() {
        assertNull("another's computer is never told", OpenRule.outcome(OpenRule.Decision.NOT_OWN_COMPUTER, false))
        assertTrue(OpenRule.outcome(OpenRule.Decision.OPEN, true)!!.ok)
        assertEquals("the app did not open", OpenRule.outcome(OpenRule.Decision.OPEN, false)!!.reason)
        assertTrue(OpenRule.outcome(OpenRule.Decision.OFFER, true)!!.ok)
        val locked = OpenRule.outcome(OpenRule.Decision.LOCKED, false)!!
        assertFalse(locked.ok)
        assertEquals("the phone is locked", locked.reason)
    }
}
