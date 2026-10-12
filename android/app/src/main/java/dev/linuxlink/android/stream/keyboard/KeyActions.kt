package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/** One thing to put on the wire. Key codes are `KeyEvent.KEYCODE_*`; the bridge maps them to evdev. */
sealed interface KeyAction {
    data class Text(val text: String) : KeyAction

    data class Tap(val keyCode: Int) : KeyAction

    /** Press [modifiers] in order, tap [keyCode], release in reverse. */
    data class Combo(val modifiers: List<Int>, val keyCode: Int) : KeyAction
}

/** How edits and keys become [KeyAction]s (spec K3, K5, K6). */
object KeyActions {
    fun forKey(keyCode: Int, modifiers: List<Modifier>): KeyAction =
        if (modifiers.isEmpty()) KeyAction.Tap(keyCode) else KeyAction.Combo(modifiers.map { it.keyCode }, keyCode)

    /**
     * Backspaces become Delete taps. Text is typed as text, except that a
     * newline is an Enter tap and a tab a Tab tap. With [modifiers] latched,
     * each character with a key of its own becomes a combo (Ctrl + c is the
     * shortcut, not the letter); one without a key is still typed.
     */
    fun forEdit(edit: Edit, modifiers: List<Modifier>): List<KeyAction> {
        val actions = mutableListOf<KeyAction>()
        repeat(edit.backspaces) { actions += forKey(KeyEvent.KEYCODE_DEL, modifiers) }
        val pending = StringBuilder()
        fun flush() {
            if (pending.isNotEmpty()) actions += KeyAction.Text(pending.toString())
            pending.setLength(0)
        }
        var index = 0
        while (index < edit.text.length) {
            val codePoint = edit.text.codePointAt(index)
            val char = String(Character.toChars(codePoint))
            index += char.length
            val keyCode = when {
                char == "\n" -> KeyEvent.KEYCODE_ENTER
                char == "\t" -> KeyEvent.KEYCODE_TAB
                modifiers.isNotEmpty() && char.length == 1 -> keyCodeFor(char[0])
                else -> null
            }
            if (keyCode == null) {
                pending.append(char)
            } else {
                flush()
                actions += forKey(keyCode, modifiers)
            }
        }
        flush()
        return actions
    }

    /** The US-layout key that types [char], if it has one of its own. */
    fun keyCodeFor(char: Char): Int? = when (char) {
        in 'a'..'z' -> KeyEvent.KEYCODE_A + (char - 'a')
        in 'A'..'Z' -> KeyEvent.KEYCODE_A + (char - 'A')
        in '0'..'9' -> KeyEvent.KEYCODE_0 + (char - '0')
        ' ' -> KeyEvent.KEYCODE_SPACE
        '-' -> KeyEvent.KEYCODE_MINUS
        '=' -> KeyEvent.KEYCODE_EQUALS
        '[' -> KeyEvent.KEYCODE_LEFT_BRACKET
        ']' -> KeyEvent.KEYCODE_RIGHT_BRACKET
        '\\' -> KeyEvent.KEYCODE_BACKSLASH
        ';' -> KeyEvent.KEYCODE_SEMICOLON
        '\'' -> KeyEvent.KEYCODE_APOSTROPHE
        ',' -> KeyEvent.KEYCODE_COMMA
        '.' -> KeyEvent.KEYCODE_PERIOD
        '/' -> KeyEvent.KEYCODE_SLASH
        '`' -> KeyEvent.KEYCODE_GRAVE
        else -> null
    }
}
