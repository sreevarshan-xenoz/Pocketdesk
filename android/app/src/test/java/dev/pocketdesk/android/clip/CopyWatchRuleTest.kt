package dev.pocketdesk.android.clip

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CopyWatchRuleTest {
    private val overlay = listOf(null, CopyWatchRule.OVERLAY_TITLE)

    private fun read(
        on: Boolean = true,
        hasComputer: Boolean = true,
        locked: Boolean = false,
        titles: List<String?> = overlay,
        nowMs: Long = 10_000,
        lastReadMs: Long = 0,
    ) = CopyWatchRule.read(on, hasComputer, locked, titles, nowMs, lastReadMs)

    @Test
    fun `a copy is read with the switch on, a computer, unlocked and the overlay up`() {
        assertTrue(read())
        assertFalse("switch off", read(on = false))
        assertFalse("no computer", read(hasComputer = false))
        assertFalse("locked", read(locked = true))
        assertFalse("no overlay: the shade or the volume panel", read(titles = listOf("NotificationShade", null)))
    }

    @Test
    fun `the overlay is known by its window's title or by SystemUI's clipboard views`() {
        // As seen on ColorOS 14: no window title, but the overlay's own views.
        assertTrue(
            read(
                titles = listOf(null, "English (UK) (QWERTY)", "com.android.systemui:id/clipboard_ui"),
            ),
        )
        assertFalse(CopyWatchRule.isOverlay(listOf("com.android.systemui:id/volume_dialog", "Google")))
        assertFalse("another app's view", CopyWatchRule.isOverlay(listOf("com.example:id/clipboard_ui")))
    }

    @Test
    fun `reads are a second and a half apart`() {
        assertFalse(read(nowMs = 10_000, lastReadMs = 9_000))
        assertTrue(read(nowMs = 10_500, lastReadMs = 9_000))
    }

    @Test
    fun `a private, blank, long or just crossed copy is not sent`() {
        assertTrue(CopyWatchRule.send("hello", private = false, lastExchanged = null))
        assertFalse("sensitive", CopyWatchRule.send("hunter2", private = true, lastExchanged = null))
        assertFalse("blank", CopyWatchRule.send("  ", private = false, lastExchanged = null))
        assertFalse("none", CopyWatchRule.send(null, private = false, lastExchanged = null))
        assertFalse("from the computer", CopyWatchRule.send("hello", private = false, lastExchanged = "hello"))
        assertFalse(
            "too long",
            CopyWatchRule.send("x".repeat(SendClipboard.MAX_CHARS + 1), private = false, lastExchanged = null),
        )
    }
}
