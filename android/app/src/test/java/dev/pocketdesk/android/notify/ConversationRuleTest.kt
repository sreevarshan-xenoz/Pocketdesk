package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Test

class ConversationRuleTest {
    @Test
    fun `the newest messages go, oldest first, none blank`() {
        val raw = (1..12).map { ConversationRule.Raw("Ann", "m$it", it * 1000L, false) } +
            ConversationRule.Raw("Ann", "   ", 99_000, false) +
            ConversationRule.Raw("Ann", null, 99_001, false)
        val out = ConversationRule.messages(raw)
        assertEquals(ConversationRule.MAX, out.size)
        assertEquals("m3", out.first().text)
        assertEquals("m12", out.last().text)
    }

    @Test
    fun `the owner's own have no sender, and long texts are cut`() {
        val out = ConversationRule.messages(
            listOf(
                ConversationRule.Raw("Me", "On my way", 1, true),
                ConversationRule.Raw(" Ann ", "x".repeat(1200), 2, false),
            ),
        )
        assertEquals(ConversationRule.Out("", "On my way", 1, true), out[0])
        assertEquals("Ann", out[1].from)
        assertEquals(ConversationRule.MAX_TEXT, out[1].text.length)
    }

    @Test
    fun `a thread with a code anywhere is not sent at all`() {
        fun thread(vararg m: ConversationRule.Raw) = ConversationRule.messages(m.toList())
        // In a message.
        assertEquals(emptyList<ConversationRule.Out>(), thread(ConversationRule.Raw("Bank", "Your verification code is 482913", 1, false)))
        // The word in the sender, the digits in the text.
        assertEquals(emptyList<ConversationRule.Out>(), thread(ConversationRule.Raw("Verification Team", "Yours: 482913", 1, false)))
        // Split over two messages from one sender.
        assertEquals(
            emptyList<ConversationRule.Out>(),
            thread(
                ConversationRule.Raw("Bank", "Your verification code is:", 1, false),
                ConversationRule.Raw("Bank", "482913", 2, false),
            ),
        )
        // An ordinary chat with numbers goes.
        assertEquals(2, thread(ConversationRule.Raw("Ann", "Meet at 1830?", 1, false), ConversationRule.Raw("Bo", "Room 4021", 2, false)).size)
    }

    @Test
    fun `a new message changes the digest, the same thread does not`() {
        assertEquals("", ConversationRule.digest(null))
        assertEquals(ConversationRule.digest("[a]"), ConversationRule.digest("[a]"))
        org.junit.Assert.assertNotEquals(ConversationRule.digest("[a]"), ConversationRule.digest("[a,b]"))
    }
}
