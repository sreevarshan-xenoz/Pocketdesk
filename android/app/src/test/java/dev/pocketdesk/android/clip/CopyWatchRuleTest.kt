package dev.pocketdesk.android.clip

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CopyWatchRuleTest {
    @Test
    fun `the overlay is known by its window's title or by SystemUI's clipboard views`() {
        // As seen on ColorOS 14: no window title, but the overlay's own views.
        assertTrue(CopyWatchRule.isOverlay(listOf(null, "com.android.systemui:id/clipboard_ui")))
        assertTrue(CopyWatchRule.isOverlayName(CopyWatchRule.OVERLAY_TITLE))
        assertFalse("the volume panel", CopyWatchRule.isOverlay(listOf("com.android.systemui:id/volume_dialog", null)))
        assertFalse("another app's view", CopyWatchRule.isOverlay(listOf("com.example:id/clipboard_ui")))
    }

    @Test
    fun `a copy within the debounce is read once it ends, never dropped`() {
        assertEquals("the first, now", 0L, CopyWatchRule.delay(nowMs = 10_000, lastReadMs = 0))
        assertEquals("long after, now", 0L, CopyWatchRule.delay(nowMs = 10_000, lastReadMs = 5_000))
        assertEquals("within it, at its end", 500L, CopyWatchRule.delay(nowMs = 10_000, lastReadMs = 9_000))
    }

    @Test
    fun `only Android 13 and later show the overlay`() {
        assertTrue(CopyWatchRule.supported(33))
        assertFalse(CopyWatchRule.supported(32))
    }

    @Test
    fun `a private, blank, long, just crossed or already read copy is not sent`() {
        assertTrue(CopyWatchRule.send("hello", private = false, lastExchanged = null))
        assertFalse("sensitive", CopyWatchRule.send("hunter2", private = true, lastExchanged = null))
        assertFalse("blank", CopyWatchRule.send("  ", private = false, lastExchanged = null))
        assertFalse("none", CopyWatchRule.send(null, private = false, lastExchanged = null))
        assertFalse("from the computer", CopyWatchRule.send("hello", private = false, lastExchanged = "hello"))
        assertFalse("read already", CopyWatchRule.send("hello", private = false, lastExchanged = null, lastRead = "hello"))
        assertFalse(
            "too long",
            CopyWatchRule.send("x".repeat(SendClipboard.MAX_CHARS + 1), private = false, lastExchanged = null),
        )
    }
}
