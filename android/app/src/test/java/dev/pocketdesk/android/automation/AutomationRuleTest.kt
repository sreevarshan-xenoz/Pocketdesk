package dev.pocketdesk.android.automation

import dev.pocketdesk.android.automation.AutomationRule.Refusal
import dev.pocketdesk.android.ui.Deck
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AutomationRuleTest {
    @Test
    fun `a name is trimmed, short and printable`() {
        assertEquals("Backup", AutomationRule.name("  Backup "))
        assertNull(AutomationRule.name(null))
        assertNull(AutomationRule.name("   "))
        assertNull(AutomationRule.name("a".repeat(AutomationRule.MAX_NAME + 1)))
        assertEquals("a".repeat(AutomationRule.MAX_NAME), AutomationRule.name("a".repeat(AutomationRule.MAX_NAME)))
        assertNull(AutomationRule.name("Back" + Char(10) + "up"))
    }

    @Test
    fun `an ask is refused in order - the computer, the name, the pace`() {
        assertEquals(Refusal.NOT_SAVED, AutomationRule.refusal(false, null, emptyList(), 0))
        assertEquals(Refusal.BAD_NAME, AutomationRule.refusal(true, null, emptyList(), 0))
        assertNull(AutomationRule.refusal(true, "Backup", emptyList(), 0))
    }

    @Test
    fun `a start uses or brings up a link only to the own computer`() {
        val own = "192.0.2.20" to 1716
        val other = "192.0.2.30" to 1716
        assertEquals(AutomationRule.Link.LINKED, AutomationRule.link(other, other, own))
        assertEquals(AutomationRule.Link.LINKED_ELSEWHERE, AutomationRule.link(own, other, own))
        assertEquals(AutomationRule.Link.CONNECT, AutomationRule.link(null, own, own))
        assertEquals(AutomationRule.Link.CONNECT, AutomationRule.link(null, other, null))
        assertEquals(AutomationRule.Link.NOT_LINKED, AutomationRule.link(null, other, own))
        // The same address on another port is another computer.
        assertEquals(AutomationRule.Link.NOT_LINKED, AutomationRule.link(null, "192.0.2.20" to 1726, own))
    }

    @Test
    fun `at most ten a minute`() {
        val now = 600_000L
        val nine = List(9) { now - 1_000L * it }
        assertNull(AutomationRule.refusal(true, "Backup", nine, now))
        assertEquals(Refusal.TOO_MANY, AutomationRule.refusal(true, "Backup", nine + (now - 59_999), now))
        // One a minute ago no longer counts, nor one from a clock gone back.
        assertNull(AutomationRule.refusal(true, "Backup", nine + (now - 60_000), now))
        assertNull(AutomationRule.refusal(true, "Backup", nine + (now + 5_000), now))
        assertEquals(listOf(now), AutomationRule.kept(listOf(now - 60_000, now), now))
    }

    @Test
    fun `a button is found by its exact label, the first of two`() {
        val buttons = listOf(Deck.Button(0, "Mute", "keys"), Deck.Button(1, "mute", "keys"), Deck.Button(2, "Mute", "task"))
        assertEquals(0, AutomationRule.button(buttons, "Mute")?.index)
        assertEquals(1, AutomationRule.button(buttons, "mute")?.index)
        assertNull(AutomationRule.button(buttons, "Unmute"))
    }

    @Test
    fun `a press is told by how it went`() {
        assertEquals(mapOf("state" to "done"), AutomationRule.pressed(Deck.outcome(true, null)))
        assertEquals(mapOf("state" to "waiting"), AutomationRule.pressed(Deck.outcome(true, null, waiting = true)))
        assertEquals(
            mapOf("state" to "refused", "reason" to "Input is off"),
            AutomationRule.pressed(Deck.outcome(false, "Input is off")),
        )
        assertEquals(mapOf("state" to "refused", "reason" to "refused"), AutomationRule.pressed(Deck.outcome(false, null)))
        assertEquals(mapOf("state" to "not_sent"), AutomationRule.pressed(null))
    }
}
