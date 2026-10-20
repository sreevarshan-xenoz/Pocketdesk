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
        assertNull("a code", DialRule.number("*#06#"))
        assertNull("call forwarding", DialRule.number("**21*5550100#"))
        assertEquals(32, DialRule.number("1".repeat(32))?.length)
        assertEquals("112", DialRule.number("112"))
        assertNull(DialRule.number("12"))
        assertNull(DialRule.number("1".repeat(33)))
        assertNull(DialRule.number("1+555"))
        assertNull(DialRule.number("555-CALL"))
    }

    /** L4: the dialer opens only with everything in place. */
    @Test
    fun `the dialer opens only when it may`() {
        assertEquals(Decision.Open, DialRule.decide(true, true, "112", true))
        assertEquals(Decision.Off, DialRule.decide(false, true, "112", true))
        assertEquals(Decision.NotOwnComputer, DialRule.decide(true, false, "112", true))
        assertEquals(Decision.NotANumber, DialRule.decide(true, true, null, true))
        assertEquals(Decision.TooMany, DialRule.decide(true, true, "112", false))
        assertEquals("tel:+15550100100", DialRule.telUri("+15550100100"))
    }
}
