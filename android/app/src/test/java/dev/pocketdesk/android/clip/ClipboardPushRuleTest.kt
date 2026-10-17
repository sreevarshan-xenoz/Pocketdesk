package dev.pocketdesk.android.clip

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ClipboardPushRuleTest {
    @Test
    fun `text from the computer is applied once, and never what just crossed`() {
        assertTrue(ClipboardPushRule.shouldApply("copied", lastExchanged = null))
        assertFalse(ClipboardPushRule.shouldApply("copied", lastExchanged = "copied"))
        assertTrue(ClipboardPushRule.shouldApply("new", lastExchanged = "copied"))
        assertFalse(ClipboardPushRule.shouldApply("   ", lastExchanged = null))
    }

    @Test
    fun `only text from the computer the phone sends to`() {
        assertTrue(ClipboardPushRule.fromOwnComputer("192.0.2.7", "192.0.2.7"))
        assertFalse(ClipboardPushRule.fromOwnComputer("198.51.100.3", "192.0.2.7"))
        assertFalse(ClipboardPushRule.fromOwnComputer("192.0.2.7", null))
    }

    @Test
    fun `send clipboard says why when it cannot`() {
        assertEquals(SendClipboard.Check.NoComputer, SendClipboard.check("hello", hasComputer = false))
        assertEquals(SendClipboard.Check.Empty, SendClipboard.check(null, hasComputer = true))
        assertEquals(SendClipboard.Check.Empty, SendClipboard.check("  ", hasComputer = true))
        assertEquals(
            SendClipboard.Check.TooLong,
            SendClipboard.check("x".repeat(SendClipboard.MAX_CHARS + 1), hasComputer = true),
        )
        assertEquals(SendClipboard.Check.Send, SendClipboard.check("hello", hasComputer = true))
    }

    @Test
    fun `a private copy is sent only once the person says so`() {
        assertEquals(SendClipboard.Check.AskPrivate, SendClipboard.check("hunter2", hasComputer = true, private = true))
        assertEquals(
            "nothing to ask about without a computer",
            SendClipboard.Check.NoComputer,
            SendClipboard.check("hunter2", hasComputer = false, private = true),
        )
    }
}
