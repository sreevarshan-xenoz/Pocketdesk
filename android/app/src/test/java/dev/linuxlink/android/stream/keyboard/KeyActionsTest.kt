package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeyActionsTest {
    private val ctrl = KeyModifier.Ctrl.keyCode
    private val shift = KeyModifier.Shift.keyCode

    @Test
    fun plainTextIsTextAndBackspacesAreDeleteTaps() {
        assertEquals(
            listOf(KeyAction.Tap(KeyEvent.KEYCODE_DEL), KeyAction.Tap(KeyEvent.KEYCODE_DEL), KeyAction.Text("he")),
            KeyActions.forEdit(Edit(2, "he"), emptyList()),
        )
    }

    @Test
    fun newlinesAndTabsAreKeys() {
        assertEquals(
            listOf(
                KeyAction.Text("a"),
                KeyAction.Tap(KeyEvent.KEYCODE_ENTER),
                KeyAction.Text("b"),
                KeyAction.Tap(KeyEvent.KEYCODE_TAB),
            ),
            KeyActions.forEdit(Edit(0, "a\nb\t"), emptyList()),
        )
    }

    @Test
    fun latchedModifiersTurnTypedCharactersIntoCombos() {
        assertEquals(
            listOf(KeyAction.Combo(listOf(ctrl), KeyEvent.KEYCODE_C)),
            KeyActions.forEdit(Edit(0, "c"), listOf(KeyModifier.Ctrl)),
        )
        assertEquals(
            "Shift+Enter, e.g. a newline in a chat box",
            listOf(KeyAction.Combo(listOf(shift), KeyEvent.KEYCODE_ENTER)),
            KeyActions.forEdit(Edit(0, "\n"), listOf(KeyModifier.Shift)),
        )
        assertEquals(
            "a character with no key of its own is still typed",
            listOf(KeyAction.Text("é")),
            KeyActions.forEdit(Edit(0, "é"), listOf(KeyModifier.Ctrl)),
        )
    }

    @Test
    fun modifiersApplyToTheFirstKeyOfAnEditOnly() {
        assertEquals(
            listOf(KeyAction.Combo(listOf(ctrl), KeyEvent.KEYCODE_W), KeyAction.Text("hat")),
            KeyActions.forEdit(Edit(0, "what"), listOf(KeyModifier.Ctrl)),
        )
        assertEquals(
            "a replacement's backspaces are bookkeeping, never shortcuts",
            listOf(KeyAction.Tap(KeyEvent.KEYCODE_DEL), KeyAction.Combo(listOf(ctrl), KeyEvent.KEYCODE_X)),
            KeyActions.forEdit(Edit(1, "x"), listOf(KeyModifier.Ctrl)),
        )
        assertEquals(
            "a plain delete is the user's key: Ctrl+Backspace once",
            listOf(KeyAction.Combo(listOf(ctrl), KeyEvent.KEYCODE_DEL), KeyAction.Tap(KeyEvent.KEYCODE_DEL)),
            KeyActions.forEdit(Edit(2, ""), listOf(KeyModifier.Ctrl)),
        )
    }

    @Test
    fun aKeyWithModifiersIsACombo() {
        assertEquals(KeyAction.Tap(KeyEvent.KEYCODE_ESCAPE), KeyActions.forKey(KeyEvent.KEYCODE_ESCAPE, emptyList()))
        assertEquals(
            KeyAction.Combo(listOf(ctrl, shift), KeyEvent.KEYCODE_TAB),
            KeyActions.forKey(KeyEvent.KEYCODE_TAB, listOf(KeyModifier.Ctrl, KeyModifier.Shift)),
        )
    }

    @Test
    fun charactersMapToTheirKeys() {
        assertEquals(KeyEvent.KEYCODE_A, KeyActions.keyCodeFor('a'))
        assertEquals(KeyEvent.KEYCODE_Z, KeyActions.keyCodeFor('Z'))
        assertEquals(KeyEvent.KEYCODE_7, KeyActions.keyCodeFor('7'))
        assertEquals(KeyEvent.KEYCODE_SPACE, KeyActions.keyCodeFor(' '))
        assertEquals(KeyEvent.KEYCODE_SLASH, KeyActions.keyCodeFor('/'))
        assertEquals(KeyEvent.KEYCODE_BACKSLASH, KeyActions.keyCodeFor('\\'))
        assertNull(KeyActions.keyCodeFor('é'))
    }
}
