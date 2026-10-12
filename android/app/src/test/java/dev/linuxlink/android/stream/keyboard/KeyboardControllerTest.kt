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
        latch.tap(KeyModifier.Ctrl, nowMs = 0)
        keyboard.setComposing("c")
        assertEquals(listOf(KeyAction.Combo(listOf(KeyModifier.Ctrl.keyCode), KeyEvent.KEYCODE_C)), sent)
        assertEquals(1, imeResets)
        assertEquals("the latch was used up", LatchState.Off, latch.state(KeyModifier.Ctrl))
        keyboard.setComposing("x")
        assertEquals("the IME's composing word was forgotten", KeyAction.Text("x"), sent.last())
    }

    @Test
    fun barKeysUseAndConsumeTheLatch() {
        latch.tap(KeyModifier.Alt, nowMs = 0)
        keyboard.key(KeyEvent.KEYCODE_TAB)
        keyboard.key(KeyEvent.KEYCODE_TAB)
        assertEquals(
            listOf(KeyAction.Combo(listOf(KeyModifier.Alt.keyCode), KeyEvent.KEYCODE_TAB), KeyAction.Tap(KeyEvent.KEYCODE_TAB)),
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
    fun aRecomposeTheShadowCannotHonourResyncsInsteadOfDuplicating() {
        keyboard.commit("hello ")
        keyboard.recompose(start = 100, end = 105)
        assertEquals(1, imeResets)
        sent.clear()
        keyboard.setComposing("worl")
        assertEquals("diffed against nothing, not a stale word", listOf(KeyAction.Text("worl")), sent)
    }

    @Test
    fun movingTheCaretResyncsTheIme() {
        keyboard.setComposing("hel")
        keyboard.key(KeyEvent.KEYCODE_DPAD_LEFT)
        assertEquals(1, imeResets)
        keyboard.setComposing("p")
        assertEquals(KeyAction.Text("p"), sent.last())
        keyboard.selectionMoved(start = 0, end = 0)
        assertEquals("a selection away from the end resyncs", 2, imeResets)
        keyboard.contextLost()
        assertEquals(3, imeResets)
    }

    @Test
    fun imeKeyEventsKeepTheShadowInStep() {
        keyboard.commit("ab")
        keyboard.hardwareKey(KeyEvent.KEYCODE_DEL, null, held = emptyList())
        assertEquals("a", keyboard.textBefore(10))
        keyboard.hardwareKey(KeyEvent.KEYCODE_ENTER, null, held = emptyList())
        assertEquals("a\n", keyboard.textBefore(10))
        assertEquals(0, imeResets)
    }

    @Test
    fun aOnceLatchIsOneKeyEvenForAWholeSwipedWord() {
        latch.tap(KeyModifier.Ctrl, nowMs = 0)
        keyboard.setComposing("what")
        assertEquals(
            listOf(KeyAction.Combo(listOf(KeyModifier.Ctrl.keyCode), KeyEvent.KEYCODE_W), KeyAction.Text("hat")),
            sent,
        )
        assertEquals(1, imeResets)
    }

    @Test
    fun anAutocorrectWithALatchSendsPlainBackspaces() {
        keyboard.setComposing("teh")
        sent.clear()
        latch.tap(KeyModifier.Ctrl, nowMs = 0)
        keyboard.commit("the ")
        assertEquals(KeyAction.Tap(KeyEvent.KEYCODE_DEL), sent.first())
        assertEquals(1, imeResets)
    }

    @Test
    fun hardwareCharactersAreTextUnlessAShortcutModifierIsHeld() {
        keyboard.hardwareKey(KeyEvent.KEYCODE_A, 'A', held = listOf(KeyModifier.Shift))
        keyboard.hardwareKey(KeyEvent.KEYCODE_C, 'c', held = listOf(KeyModifier.Ctrl))
        keyboard.hardwareKey(KeyEvent.KEYCODE_F5, null, held = emptyList())
        assertEquals(
            listOf(
                KeyAction.Text("A"),
                KeyAction.Combo(listOf(KeyModifier.Ctrl.keyCode), KeyEvent.KEYCODE_C),
                KeyAction.Tap(KeyEvent.KEYCODE_F5),
            ),
            sent,
        )
    }
}
