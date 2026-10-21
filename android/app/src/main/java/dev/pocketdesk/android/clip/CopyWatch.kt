package dev.pocketdesk.android.clip

/**
 * Copies on the phone sent by themselves (M10): when C8's Accessibility
 * service sees SystemUI's clipboard overlay come up, a copy was just made,
 * and a window that never shows reads it (Android lets only the app in
 * front read the clipboard). Pure, for JVM tests.
 */
object CopyWatchRule {
    /** The clipboard overlay's window, as SystemUI names it. */
    const val OVERLAY_TITLE = "ClipboardOverlay"

    /** SystemUI, the only package whose windows are watched. */
    const val SYSTEM_UI = "com.android.systemui"

    /** Android 13, whose SystemUI shows the clipboard overlay a copy is known by. */
    const val MIN_SDK = 33

    /** At most one read this often: the overlay redraws as it animates. */
    const val DEBOUNCE_MS = 1_500L

    /** Whether this Android shows the overlay, so the switch can work at all. */
    fun supported(sdk: Int): Boolean = sdk >= MIN_SDK

    /**
     * Whether a window's [names] (its title and its views' ids) show the
     * clipboard overlay: its window's title, or a SystemUI view of it
     * (`com.android.systemui:id/clipboard_…`), where the title is not given.
     */
    fun isOverlay(names: List<String?>): Boolean = names.any { isOverlayName(it) }

    fun isOverlayName(name: String?): Boolean =
        name == OVERLAY_TITLE || name?.startsWith("$SYSTEM_UI:id/clipboard") == true

    /**
     * When to read after the overlay came up at [nowMs], the last read
     * having been at [lastReadMs]: now (0), or after the debounce (the wait,
     * M10 review: a copy within it had been dropped, never read).
     */
    fun delay(nowMs: Long, lastReadMs: Long): Long =
        if (lastReadMs == 0L) 0L else (lastReadMs + DEBOUNCE_MS - nowMs).coerceAtLeast(0L)

    /**
     * Send [text], copied [private]ly or not? Not what last crossed between
     * the phone and the computer ([lastExchanged]), nor what was last read
     * here ([lastRead]: the overlay staying up, or a send still on its way,
     * must not send it again, M10 review).
     */
    fun send(text: String?, private: Boolean, lastExchanged: String?, lastRead: String? = null): Boolean =
        !text.isNullOrBlank() && !private && text.length <= SendClipboard.MAX_CHARS &&
            text != lastExchanged && text != lastRead
}

/** What the last read took, so it is never sent twice (M10). */
object CopyMemory {
    @Volatile
    var lastRead: String? = null
}
