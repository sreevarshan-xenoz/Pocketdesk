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
}
