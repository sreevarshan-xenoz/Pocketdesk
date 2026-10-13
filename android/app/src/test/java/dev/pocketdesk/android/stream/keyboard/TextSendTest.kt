package dev.pocketdesk.android.stream.keyboard

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TextSendTest {
    @Test
    fun linesBecomeEnterTapsAndEnterIsAddedOnlyWhenAsked() {
        assertEquals(
            listOf(KeyAction.Text("ls"), KeyAction.Tap(KeyEvent.KEYCODE_ENTER), KeyAction.Text("pwd")),
            TextSend.actions("ls\npwd", pressEnter = false).getOrThrow(),
        )
        assertEquals(
            KeyAction.Tap(KeyEvent.KEYCODE_ENTER),
            TextSend.actions("ls", pressEnter = true).getOrThrow().last(),
        )
    }

    @Test
    fun blankAndOverlongTextIsRefused() {
        assertTrue(TextSend.actions("", pressEnter = true).isFailure)
        assertTrue(TextSend.actions("x".repeat(TextSend.MAX_CHARS + 1), pressEnter = false).isFailure)
        assertTrue(TextSend.actions("x".repeat(TextSend.MAX_CHARS), pressEnter = false).isSuccess)
    }

    @Test
    fun historyIsNewestFirstDistinctAndBounded() {
        val history = SendHistory(max = 3)
        history.add("a")
        history.add("b")
        history.add("a")
        assertEquals(listOf("a", "b"), history.items())
        history.add("c")
        history.add("d")
        assertEquals(listOf("d", "c", "a"), history.items())
        history.add("   ")
        assertEquals("blank text is not history", listOf("d", "c", "a"), history.items())
    }
}
