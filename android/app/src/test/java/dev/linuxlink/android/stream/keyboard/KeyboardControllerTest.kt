package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Test

class KeyboardControllerTest {
    private val sent = mutableListOf<KeyAction>()
    private var imeResets = 0
    private val latch = ModifierLatch()
    private val keyboard = KeyboardController(latch, { sent += it }, { imeResets++ })

    @Test
    fun typingIsSentAsItIsComposed() {
        keyboard.setComposing("hi")
        keyboard.commit("hi ")
        assertEquals(listOf(KeyAction.Text("hi"), KeyAction.Text(" ")), sent)
    }

    @Test
    fun aLatchedCtrlTurnsTheNextLetterIntoAShortcutAndResetsTheIme() {
        latch.tap(Modifier.Ctrl)
        keyboard.setComposing("c")
        assertEquals(listOf(KeyAction.Combo(listOf(Modifier.Ctrl.keyCode), KeyEvent.KEYCODE_C)), sent)
        assertEquals(1, imeResets)
        assertEquals("the latch was used up", LatchState.Off, latch.state(Modifier.Ctrl))
        keyboard.setComposing("x")
        assertEquals("the IME's composing word was forgotten", KeyAction.Text("x"), sent.last())
    }

    @Test
    fun barKeysUseAndConsumeTheLatch() {
        latch.tap(Modifier.Alt)
        keyboard.key(KeyEvent.KEYCODE_TAB)
        keyboard.key(KeyEvent.KEYCODE_TAB)
        assertEquals(
            listOf(KeyAction.Combo(listOf(Modifier.Alt.keyCode), KeyEvent.KEYCODE_TAB), KeyAction.Tap(KeyEvent.KEYCODE_TAB)),
            sent,
        )
    }

    @Test
    fun aDeleteKeyFromTheImeAlsoShortensTheShadow() {
        keyboard.commit("ab")
        keyboard.key(KeyEvent.KEYCODE_DEL)
        assertEquals("a", keyboard.textBefore(10))
        assertEquals(KeyAction.Tap(KeyEvent.KEYCODE_DEL), sent.last())
    }

    @Test
    fun hardwareCharactersAreTextUnlessAShortcutModifierIsHeld() {
        keyboard.hardwareKey(KeyEvent.KEYCODE_A, 'A', held = listOf(Modifier.Shift))
        keyboard.hardwareKey(KeyEvent.KEYCODE_C, 'c', held = listOf(Modifier.Ctrl))
        keyboard.hardwareKey(KeyEvent.KEYCODE_F5, null, held = emptyList())
        assertEquals(
            listOf(
                KeyAction.Text("A"),
                KeyAction.Combo(listOf(Modifier.Ctrl.keyCode), KeyEvent.KEYCODE_C),
                KeyAction.Tap(KeyEvent.KEYCODE_F5),
            ),
            sent,
        )
    }
}
