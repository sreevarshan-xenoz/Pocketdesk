package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.RecheckPlan.Act
import org.junit.Assert.assertEquals
import org.junit.Test

class RecheckPlanTest {
    private val five = RecheckPlan.EVERY_MS

    /** BB2: a computer that refused is dialed every few minutes, with no link up. */
    @Test
    fun `it is looked at again every few minutes`() {
        assertEquals(Act.DIAL, RecheckPlan.act(null, "a", false, five, 0, null))
        assertEquals(Act.NONE, RecheckPlan.act(null, "a", false, five - 1, 0, null))
        assertEquals(Act.NONE, RecheckPlan.act(null, null, false, five, 0, null))
    }

    /** Review: a link it did not open, the person's own session or pairing, is never closed. */
    @Test
    fun `only a link the look opened is let go`() {
        val later = RecheckPlan.HOLD_MS
        assertEquals(Act.CLOSE, RecheckPlan.act("a", "a", true, later, 0, null))
        assertEquals(Act.NONE, RecheckPlan.act("a", "a", false, later, 0, null))
        assertEquals("within its hold", Act.NONE, RecheckPlan.act("a", "a", true, later - 1, 0, null))
        assertEquals("another computer's link", Act.NONE, RecheckPlan.act("b", "a", true, later, 0, null))
    }

    /** Review: the own computer's link, down a moment, is its own to take back. */
    @Test
    fun `the own computer's link comes first`() {
        assertEquals(Act.NONE, RecheckPlan.act(null, "a", false, five, 0, RecheckPlan.OWN_GRACE_MS - 1))
        assertEquals(Act.DIAL, RecheckPlan.act(null, "a", false, five, 0, RecheckPlan.OWN_GRACE_MS))
    }
}
