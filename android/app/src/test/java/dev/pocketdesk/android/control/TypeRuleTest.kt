package dev.pocketdesk.android.control

import org.junit.Assert.assertEquals
import org.junit.Test

class TypeRuleTest {
    private fun decide(
        on: Boolean = true,
        service: Boolean = true,
        own: Boolean = true,
        stoppedUntilMs: Long = 0,
        nowMs: Long = 1_000_000,
    ) = TypeRule.decide(on, service, own, stoppedUntilMs, nowMs)

    @Test
    fun `typed from its own computer with control on and the service running`() {
        assertEquals(TypeRule.Decision.TYPE, decide())
    }

    @Test
    fun `not with control off, from another computer, or without the service`() {
        assertEquals(TypeRule.Decision.OFF, decide(on = false))
        assertEquals(TypeRule.Decision.NOT_OWN_COMPUTER, decide(own = false))
        assertEquals(TypeRule.Decision.SERVICE_OFF, decide(service = false))
    }

    @Test
    fun `Stop refuses typing for ten minutes`() {
        val now = 1_000_000L
        assertEquals(TypeRule.Decision.STOPPED, decide(stoppedUntilMs = now + TypeRule.STOP_MS, nowMs = now))
        assertEquals(TypeRule.Decision.TYPE, decide(stoppedUntilMs = now, nowMs = now))
    }
}
