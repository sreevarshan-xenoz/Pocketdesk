package dev.pocketdesk.android.calls

import dev.pocketdesk.android.calls.DialRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DialRuleTest {
    /** L4: numbers as the computer cleans them. */
    @Test
    fun `a number is cleaned as the computer does`() {
        assertEquals("+15550100100", DialRule.number("+1 (555) 010-0100"))
        assertEquals("*#06#", DialRule.number("*#06#"))
        assertEquals("112", DialRule.number("112"))
        assertNull(DialRule.number("12"))
        assertNull(DialRule.number("1".repeat(33)))
        assertNull(DialRule.number("1+555"))
        assertNull(DialRule.number("555-CALL"))
        assertNull(DialRule.number("***"))
    }

    /** L4: the dialer opens only with everything in place. */
    @Test
    fun `the dialer opens only when it may`() {
        assertEquals(Decision.Open, DialRule.decide(true, true, "112", true))
        assertEquals(Decision.Off, DialRule.decide(false, true, "112", true))
        assertEquals(Decision.NotOwnComputer, DialRule.decide(true, false, "112", true))
        assertEquals(Decision.NotANumber, DialRule.decide(true, true, null, true))
        assertEquals(Decision.TooMany, DialRule.decide(true, true, "112", false))
        assertEquals("tel:*%2306%23", DialRule.telUri("*#06#"))
    }
}
