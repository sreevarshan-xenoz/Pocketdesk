package dev.linuxlink.android.stream.keyboard

import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection

/**
 * An invisible, focusable view that the phone keyboard types into during a
 * session (spec K1). It has no text of its own: the IME talks to a
 * [RemoteInputConnection], and key events, from a hardware keyboard or
 * `adb shell input`, go to the same controller (spec K11).
 */
class KeyboardSinkView(context: Context) : View(context) {
    var keyboard: KeyboardController? = null

    init {
        // Focusable only while the keyboard is open (KeyboardSession), so
        // a hardware keyboard cannot reach the desktop with no bar showing.
        isFocusable = false
        isFocusableInTouchMode = false
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? {
        val keyboard = keyboard ?: return null
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
        // Remote logins and website passwords are typed through here: the
        // phone keyboard must not learn them.
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING or
            EditorInfo.IME_ACTION_NONE
        outAttrs.initialSelStart = 0
        outAttrs.initialSelEnd = 0
        return RemoteInputConnection(this, keyboard)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val keyboard = keyboard
        if (keyboard == null || keyCode in PASS_THROUGH || KeyEvent.isModifierKey(keyCode)) {
            return super.onKeyDown(keyCode, event)
        }
        val unicode = event.unicodeChar
        val char = if (unicode != 0 && !Character.isISOControl(unicode)) unicode.toChar() else null
        keyboard.hardwareKey(keyCode, char, held(event))
        return true
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        if (keyboard == null || keyCode in PASS_THROUGH) super.onKeyUp(keyCode, event) else true

    private fun held(event: KeyEvent): List<KeyModifier> = buildList {
        if (event.isCtrlPressed) add(KeyModifier.Ctrl)
        if (event.isAltPressed) add(KeyModifier.Alt)
        if (event.isShiftPressed) add(KeyModifier.Shift)
        if (event.isMetaPressed) add(KeyModifier.Super)
    }

    private companion object {
        /** Keys the phone keeps: Back closes the keyboard, the rest are the phone's own. */
        val PASS_THROUGH = setOf(
            KeyEvent.KEYCODE_BACK,
            KeyEvent.KEYCODE_HOME,
            KeyEvent.KEYCODE_VOLUME_UP,
            KeyEvent.KEYCODE_VOLUME_DOWN,
            KeyEvent.KEYCODE_VOLUME_MUTE,
            KeyEvent.KEYCODE_POWER,
            KeyEvent.KEYCODE_APP_SWITCH,
        )
    }
}
