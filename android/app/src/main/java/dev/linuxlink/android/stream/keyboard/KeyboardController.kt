package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/**
 * The keyboard's logic between the phone and the wire, with no Android
 * views in it: the input connection, the bar and hardware keys all call in
 * here on the main thread, and it hands [KeyAction]s to [send], which must
 * deliver them in order. [resetIme] makes the IME forget its composing state
 * (an `InputMethodManager.restartInput`).
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

    fun recompose(start: Int, end: Int) = shadow.recompose(start, end)

    fun textBefore(n: Int): String = shadow.before(n)

    /** A key from the bar, or a non-text key from the IME, with the latched modifiers. */
    fun key(keyCode: Int) {
        when (keyCode) {
            KeyEvent.KEYCODE_DEL -> shadow.deleteBefore(1)
            KeyEvent.KEYCODE_ENTER -> {
                shadow.finishComposing()
                shadow.commit("\n")
            }
        }
        send(listOf(KeyActions.forKey(keyCode, latch.consume())))
    }

    /**
     * A key from a hardware keyboard (or `adb shell input`). [char] is what
     * it types with [held] applied, or null for a non-printing key. A
     * printable character is typed as text unless a shortcut modifier (not
     * Shift alone) is held or latched.
     */
    fun hardwareKey(keyCode: Int, char: Char?, held: List<Modifier>) {
        val modifiers = (held + latch.consume()).distinct().sortedBy { it.ordinal }
        val shortcut = modifiers.any { it != Modifier.Shift }
        if (char != null && !shortcut) {
            shadow.finishComposing()
            shadow.commit(char.toString())
            send(listOf(KeyAction.Text(char.toString())))
        } else {
            send(listOf(KeyActions.forKey(keyCode, modifiers)))
        }
    }

    /** Forget the field's contents, e.g. when the keyboard opens again. */
    fun reset() {
        shadow.clear()
    }

    private fun emit(edit: Edit) {
        if (edit.isEmpty) return
        val modifiers = latch.consume()
        send(KeyActions.forEdit(edit, modifiers))
        if (modifiers.isNotEmpty() && edit.text.isNotEmpty()) {
            // The desktop got shortcuts, not the characters the IME thinks it
            // typed; make it start over so its next edit deletes nothing.
            shadow.clear()
            resetIme()
        }
    }
}
