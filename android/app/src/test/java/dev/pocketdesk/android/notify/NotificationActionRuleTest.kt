package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NotificationActionRuleTest {
    private val reply = NotificationActionRule.ActionInfo("Reply", takesText = true)

    @Test
    fun `plain buttons with usable titles are offered, three at most, and the reply apart`() {
        val actions =
            listOf(
                reply,
                NotificationActionRule.ActionInfo("Mark as read", takesText = false),
                NotificationActionRule.ActionInfo(null, takesText = false),
                NotificationActionRule.ActionInfo("  ", takesText = false),
                NotificationActionRule.ActionInfo("x".repeat(41), takesText = false),
                NotificationActionRule.ActionInfo("Mute", takesText = false),
                NotificationActionRule.ActionInfo("Mute", takesText = false),
                NotificationActionRule.ActionInfo("Archive", takesText = false),
                NotificationActionRule.ActionInfo("Delete", takesText = false),
            )
        assertEquals(listOf("Mark as read", "Mute", "Archive"), NotificationActionRule.offered(actions))
        assertEquals(0, NotificationActionRule.replyIndex(actions))
        assertNull(NotificationActionRule.replyIndex(actions.drop(1)))
    }

    @Test
    fun `a button that needs the phone unlocked, or has nothing to run, is never offered`() {
        val actions =
            listOf(
                NotificationActionRule.ActionInfo("Approve", takesText = false, usable = false),
                NotificationActionRule.ActionInfo("Reply", takesText = true, usable = false),
                NotificationActionRule.ActionInfo("Snooze", takesText = false),
            )
        assertEquals(listOf("Snooze"), NotificationActionRule.offered(actions))
        assertNull(NotificationActionRule.replyIndex(actions))
    }

    @Test
    fun `answers come only from the computer the phone sends to`() {
        assertEquals(true, NotificationActionRule.fromOwnComputer("192.0.2.7", 1716, "192.0.2.7", 1716))
        assertEquals(false, NotificationActionRule.fromOwnComputer("192.0.2.7", 1717, "192.0.2.7", 1716))
        assertEquals(false, NotificationActionRule.fromOwnComputer("198.51.100.3", 1716, "192.0.2.7", 1716))
        assertEquals(false, NotificationActionRule.fromOwnComputer("192.0.2.7", 1716, null, null))
    }

    @Test
    fun `the signature of what is offered changes with the buttons`() {
        val a = NotificationActionRule.signature(listOf("Mark as read"), reply = true)
        assertEquals(a, NotificationActionRule.signature(listOf("Mark as read"), reply = true))
        assert(a != NotificationActionRule.signature(listOf("Mark as read"), reply = false))
        assert(a != NotificationActionRule.signature(listOf("Archive"), reply = true))
        assertEquals("", NotificationActionRule.signature(emptyList(), reply = false))
    }

    @Test
    fun `an answer runs only when every gate is open`() {
        fun decide(
            on: Boolean = true,
            own: Boolean = true,
            known: Boolean = true,
            offered: Boolean = true,
            budget: Boolean = true,
        ) = NotificationActionRule.decide(on, own, known, offered, budget)
        assertEquals(NotificationActionRule.Decision.Run, decide())
        assertEquals(NotificationActionRule.Decision.Off, decide(on = false))
        assertEquals(NotificationActionRule.Decision.NotOwnComputer, decide(own = false))
        assertEquals(NotificationActionRule.Decision.Gone, decide(known = false))
        assertEquals(NotificationActionRule.Decision.NotOffered, decide(offered = false))
        assertEquals(NotificationActionRule.Decision.TooMany, decide(budget = false))
    }

    @Test
    fun `at most ten answers a minute`() {
        val budget = NotificationActionRule.Budget()
        repeat(10) { assertEquals(true, budget.take(1_000L + it)) }
        assertEquals(false, budget.take(2_000L))
        assertEquals("a minute after the first", true, budget.take(61_001L))
    }

    @Test
    fun `each answer says how it went, never to another computer`() {

        assertNull(NotificationActionRule.notRun(NotificationActionRule.Decision.Run))
        assertNull("another computer is not told", NotificationActionRule.notRun(NotificationActionRule.Decision.NotOwnComputer))
        assertEquals(false, NotificationActionRule.notRun(NotificationActionRule.Decision.Gone)!!.ok)
        assertEquals("answering is off on the phone", NotificationActionRule.notRun(NotificationActionRule.Decision.Off)!!.reason)
        assertEquals("too many answers at once", NotificationActionRule.notRun(NotificationActionRule.Decision.TooMany)!!.reason)
        assertEquals(NotificationActionRule.Outcome(true, null), NotificationActionRule.ran(failed = false, refused = false))
        assertEquals("the app refused it", NotificationActionRule.ran(failed = true, refused = true).reason)
        assertEquals("the phone could not run it", NotificationActionRule.ran(failed = true, refused = false).reason)
    }
}
