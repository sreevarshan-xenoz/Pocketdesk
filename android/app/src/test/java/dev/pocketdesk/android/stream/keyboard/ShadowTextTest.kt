package dev.pocketdesk.android.stream.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ShadowTextTest {
    @Test
    fun aComposedWordIsSentLetterByLetter() {
        val shadow = ShadowText()
        assertEquals(Edit(0, "h"), shadow.setComposing("h"))
        assertEquals(Edit(0, "e"), shadow.setComposing("he"))
        assertEquals(Edit(0, "y"), shadow.setComposing("hey"))
        assertEquals("committing the same word sends nothing more", Edit(0, " "), shadow.commit("hey "))
        assertEquals("hey ", shadow.all())
    }

    @Test
    fun autocorrectReplacesOnlyTheDifferingTail() {
        val shadow = ShadowText()
        shadow.setComposing("teh")
        assertEquals(Edit(2, "he "), shadow.commit("the "))
        assertEquals("the ", shadow.all())
    }

    @Test
    fun aSwipedWordArrivesWhole() {
        val shadow = ShadowText()
        assertEquals(Edit(0, "hello"), shadow.setComposing("hello"))
        shadow.finishComposing()
        assertEquals(Edit(0, " "), shadow.commit(" "))
        assertEquals("hello ", shadow.all())
    }

    @Test
    fun deletesBeforeTheCursorAreBackspaces() {
        val shadow = ShadowText()
        shadow.commit("abc")
        shadow.setComposing("de")
        assertEquals(Edit(3, ""), shadow.deleteBefore(3))
        assertEquals("ab", shadow.all())
        assertEquals("deleting more than there is", Edit(5, ""), shadow.deleteBefore(5))
        assertEquals("", shadow.all())
    }

    @Test
    fun anEmojiIsOneBackspace() {
        val shadow = ShadowText()
        shadow.commit("a😀") // a😀
        assertEquals(Edit(1, ""), shadow.deleteBefore(2))
        assertEquals("a", shadow.all())
        shadow.setComposing("😀")
        assertEquals("replacing an emoji with another", Edit(1, "😁"), shadow.setComposing("😁"))
    }

    @Test
    fun theLastWordCanBeComposedAgain() {
        val shadow = ShadowText()
        shadow.commit("one two")
        assertTrue(shadow.recompose(start = 4, end = 7))
        assertEquals(Edit(2, "oo"), shadow.setComposing("too"))
        assertEquals("one too", shadow.all())
        assertFalse("not at the end of the text", shadow.recompose(start = 0, end = 3))
    }

    @Test
    fun theTextBeforeTheCursorIsAvailableAfterTrimming() {
        val shadow = ShadowText(keep = 8)
        shadow.commit("0123456789abcdef")
        assertEquals("cdef", shadow.before(4))
        assertEquals(8, shadow.before(100).length)
        assertTrue("indices still count from the start", shadow.recompose(start = 14, end = 16))
        assertEquals(Edit(1, "F"), shadow.setComposing("eF"))
    }

    @Test
    fun afterAClearTheRestartedImeCountsFromZeroAgain() {
        // clear() pairs with restartInput, which starts the IME at offset 0.
        val shadow = ShadowText()
        shadow.commit("hello ")
        shadow.clear()
        shadow.commit("world")
        assertTrue(shadow.recompose(start = 0, end = 5))
        assertEquals(Edit(1, ""), shadow.setComposing("worl"))
        assertEquals("the cursor in the IME's own count", 4, shadow.cursor())
    }

    @Test
    fun clearForgetsEverything() {
        val shadow = ShadowText()
        shadow.commit("abc")
        shadow.setComposing("d")
        shadow.clear()
        assertEquals("", shadow.all())
        assertEquals(Edit(0, "x"), shadow.setComposing("x"))
    }
}
