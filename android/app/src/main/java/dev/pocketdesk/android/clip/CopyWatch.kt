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

    /** At most one read this often: the overlay redraws as it animates. */
    const val DEBOUNCE_MS = 1_500L

    /**
     * Whether a window's [names] (its windows' titles and its views' ids)
     * show the clipboard overlay: its window's title, or a SystemUI view of
     * it (`com.android.systemui:id/clipboard_…`), where the title is not given.
     */
    fun isOverlay(names: List<String?>): Boolean =
        names.any { it == OVERLAY_TITLE || it?.startsWith("$SYSTEM_UI:id/clipboard") == true }

    /**
     * Read the clipboard now? With the switch [on], a computer to send to,
     * the phone not [locked], the overlay among the [titles] (window titles
     * and view ids, [isOverlay]), and the last read at least [DEBOUNCE_MS] ago.
     */
    fun read(on: Boolean, hasComputer: Boolean, locked: Boolean, titles: List<String?>, nowMs: Long, lastReadMs: Long): Boolean =
        on && hasComputer && !locked && isOverlay(titles) &&
            (lastReadMs == 0L || nowMs - lastReadMs >= DEBOUNCE_MS)

    /** Send [text], copied [private]ly or not, given what last crossed ([lastExchanged])? */
    fun send(text: String?, private: Boolean, lastExchanged: String?): Boolean =
        !text.isNullOrBlank() && !private && text.length <= SendClipboard.MAX_CHARS && text != lastExchanged
}
