package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/**
 * The keyboard's logic between the phone and the wire, with no Android
 * views in it. The input connection, the bar and hardware keys all call in
 * here on the main thread, and it hands [KeyAction]s to [send], which must
 * deliver them in order. [resetIme] makes the IME forget its composing state
 * (an `InputMethodManager.restartInput`).
 *
 * The shadow model assumes the desktop's caret is at the end of what was
 * typed. Whenever that may stop being true, it resyncs: the shadow is
 * cleared and the IME restarted, so the IME's next edit cannot send
 * backspaces against text that is no longer before the caret. That happens
 * on a caret-moving key, a shortcut, a selection away from the end, a
 * composing region the shadow cannot honour, or a touch on the desktop
 * ([contextLost]).
 */
class KeyboardController(
    private val latch: ModifierLatch,
    private val send: (List<KeyAction>) -> Unit,
    private val resetIme: () -> Unit,
) {
    private val shadow = ShadowText()

    fun setComposing(text: CharSequence) = emit(shadow.setComposing(text.toString()))

    fun commit(text: CharSequence) = emit(shadow.commit(text.toString()))

    fun finishComposing() = shadow.finishComposing()

    fun deleteBefore(chars: Int) = emit(shadow.deleteBefore(chars))

    fun deleteBeforeCodePoints(count: Int) = emit(shadow.deleteBeforeCodePoints(count))

    /**
     * The IME re-composes a region, e.g. on backspacing into the last word.
     * One the shadow cannot take is not ignored: the IME's next edit would
     * assume it replaced that region and duplicate text, so it resyncs.
     */
    fun recompose(start: Int, end: Int) {
        if (!shadow.recompose(start, end)) resync()
    }

    /** The IME moved the selection. Anywhere but a cursor at the end resyncs. */
    fun selectionMoved(start: Int, end: Int) {
        if (start != end || end != shadow.cursor()) resync()
    }

    /** Something outside the keyboard (a touch on the desktop) may have moved the caret. */
    fun contextLost() = resync()

    fun textBefore(n: Int): String = shadow.before(n)

    /** A key from the bar, or from the IME's editor action, with the latched modifiers. */
    fun key(keyCode: Int) = sendKey(keyCode, latch.consume())

    /**
     * A key from a hardware keyboard, or an IME that sends keys (Backspace,
     * Enter), or `adb shell input`. [char] is what it types with [held]
     * applied, or null for a non-printing key. A printable character is
     * typed as text unless a shortcut modifier (not Shift alone) is held or
     * latched.
     */
    fun hardwareKey(keyCode: Int, char: Char?, held: List<KeyModifier>) {
        val modifiers = (held + latch.consume()).distinct().sortedBy { it.ordinal }
        val shortcut = modifiers.any { it != KeyModifier.Shift }
        if (char != null && !shortcut) {
            shadow.finishComposing()
            shadow.commit(char.toString())
            send(listOf(KeyAction.Text(char.toString())))
        } else {
            sendKey(keyCode, modifiers)
        }
    }

    /** Forget the field's contents, e.g. when the keyboard opens again. */
    fun reset() {
        shadow.clear()
    }

    private fun sendKey(keyCode: Int, modifiers: List<KeyModifier>) {
        send(listOf(KeyActions.forKey(keyCode, modifiers)))
        when {
            // Anything with a modifier is a shortcut: what it did to the field is unknowable.
            modifiers.isNotEmpty() -> resync()
            keyCode == KeyEvent.KEYCODE_DEL -> shadow.deleteBefore(1)
            keyCode == KeyEvent.KEYCODE_ENTER -> {
                shadow.finishComposing()
                shadow.commit("\n")
            }
            // Arrows, Home/End, Tab, Esc, F-keys: the caret may be anywhere now.
            else -> resync()
        }
    }

    private fun emit(edit: Edit) {
        if (edit.isEmpty) return
        val modifiers = latch.consume()
        send(KeyActions.forEdit(edit, modifiers))
        // The desktop got a shortcut, not what the IME thinks it typed.
        if (modifiers.isNotEmpty()) resync()
    }

    private fun resync() {
        shadow.clear()
        resetIme()
    }
}
