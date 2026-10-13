package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/**
 * Text sent as a whole (roadmap F2): from the Send text sheet, or the phone
 * clipboard typed as keystrokes for fields that block paste. It types
 * exactly as live typing of the same text would (spec S2): text as text,
 * `\n` as Enter, `\t` as Tab, no modifiers.
 */
object TextSend {
    /** Longer is refused (spec S5): one input packet is read up to 64 KB. */
    const val MAX_CHARS = 4_000

    fun actions(text: String, pressEnter: Boolean): Result<List<KeyAction>> = runCatching {
        require(text.isNotEmpty()) { "nothing to send" }
        require(text.length <= MAX_CHARS) { "longer than $MAX_CHARS characters" }
        val actions = KeyActions.forEdit(Edit(0, text), emptyList())
        if (pressEnter) actions + KeyAction.Tap(KeyEvent.KEYCODE_ENTER) else actions
    }
}

/**
 * The last texts sent, newest first and distinct (spec S4). Memory only: it
 * goes with the app's process, because what was typed may have been a
 * password.
 */
class SendHistory(private val max: Int = 10) {
    private val items = ArrayDeque<String>()

    fun add(text: String) {
        if (text.isBlank()) return
        items.remove(text)
        items.addFirst(text)
        while (items.size > max) items.removeLast()
    }

    fun items(): List<String> = items.toList()
}
