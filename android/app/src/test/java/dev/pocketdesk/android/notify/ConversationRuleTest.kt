package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Test

class ConversationRuleTest {
    private val hidden = "(a code, not sent)"

    @Test
    fun `the newest messages go, oldest first, none blank`() {
        val raw = (1..12).map { ConversationRule.Raw("Ann", "m$it", it * 1000L, false) } +
            ConversationRule.Raw("Ann", "   ", 99_000, false) +
            ConversationRule.Raw("Ann", null, 99_001, false)
        val out = ConversationRule.messages(raw, hidden)
        assertEquals(ConversationRule.MAX, out.size)
        assertEquals("m3", out.first().text)
        assertEquals("m12", out.last().text)
    }

    @Test
    fun `the owner's own have no sender, and a code is never sent`() {
        val out = ConversationRule.messages(
            listOf(
                ConversationRule.Raw("Me", "On my way", 1, true),
                ConversationRule.Raw(" Bank ", "Your verification code is 482913", 2, false),
            ),
            hidden,
        )
        assertEquals(ConversationRule.Out("", "On my way", 1, true), out[0])
        assertEquals("Bank", out[1].from)
        assertEquals(hidden, out[1].text)
    }

    @Test
    fun `a new message changes the digest, the same thread does not`() {
        assertEquals("", ConversationRule.digest(null))
        assertEquals(ConversationRule.digest("[a]"), ConversationRule.digest("[a]"))
        org.junit.Assert.assertNotEquals(ConversationRule.digest("[a]"), ConversationRule.digest("[a,b]"))
    }
}
