package dev.linuxlink.android.stream.keyboard

import android.text.TextUtils
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.CompletionInfo
import android.view.inputmethod.CorrectionInfo
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest

/**
 * The IME's side of the in-session keyboard: every edit the phone keyboard
 * makes goes to [keyboard], which turns it into keys and text for the
 * desktop (spec K1-K4). There is no editable text here; the IME's view of
 * the field is [KeyboardController]'s shadow model, and the cursor is always
 * at its end. Key events (Enter from some keyboards, Backspace on an empty
 * field) are dispatched to the sink view by the base class.
 */
class RemoteInputConnection(
    view: View,
    private val keyboard: KeyboardController,
) : BaseInputConnection(view, false) {
    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        keyboard.commit(text ?: "")
        return true
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        keyboard.setComposing(text ?: "")
        return true
    }

    override fun finishComposingText(): Boolean {
        keyboard.finishComposing()
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        keyboard.recompose(minOf(start, end), maxOf(start, end))
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        // Nothing is ever after the cursor.
        if (beforeLength > 0) keyboard.deleteBefore(beforeLength)
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean {
        if (beforeLength > 0) keyboard.deleteBeforeCodePoints(beforeLength)
        return true
    }

    override fun commitCompletion(text: CompletionInfo?): Boolean {
        text?.text?.let { keyboard.commit(it) }
        return true
    }

    override fun commitCorrection(correctionInfo: CorrectionInfo?): Boolean = true

    override fun performEditorAction(actionCode: Int): Boolean {
        keyboard.key(KeyEvent.KEYCODE_ENTER)
        return true
    }

    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = keyboard.textBefore(n)

    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""

    override fun getSelectedText(flags: Int): CharSequence? = null

    override fun getCursorCapsMode(reqModes: Int): Int {
        val before = keyboard.textBefore(CAPS_CONTEXT)
        return TextUtils.getCapsMode(before, before.length, reqModes)
    }

    override fun setSelection(start: Int, end: Int): Boolean {
        keyboard.selectionMoved(start, end)
        return true
    }

    override fun getExtractedText(request: ExtractedTextRequest?, flags: Int): ExtractedText? = null

    private companion object {
        /** Enough to see the end of the last sentence. */
        const val CAPS_CONTEXT = 64
    }
}
