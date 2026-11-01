package dev.pocketdesk.watchlink

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WatchTextTest {
    @Test
    fun controlAndDirectionCharactersAreTakenOut() {
        assertEquals("sudo ls", WatchText.clean("sudo\u0000 ls‮", 50))
        assertEquals("a b", WatchText.clean("a\nb", 50))
        assertEquals("ab", WatchText.clean("a​b", 50))
        assertEquals("ab", WatchText.clean("  a b  ", 50))
    }

    @Test
    fun aLongTextIsCutWithAnEllipsisAtACharacter() {
        assertEquals("abcd…", WatchText.clean("abcdefgh", 5))
        assertEquals("abcde", WatchText.clean("abcde", 5))
        // A pair of surrogates is one character and is never cut in two.
        val emoji = "😀"
        assertEquals("a$emoji…", WatchText.clean("a$emoji$emoji$emoji", 3))
    }

    @Test
    fun aStringFitsAFrameWithoutCuttingACharacter() {
        val long = "é".repeat(200)
        val fit = WatchText.fit(long)
        assertTrue(fit.toByteArray(Charsets.UTF_8).size <= WatchText.MAX_BYTES)
        assertTrue(fit.all { it == 'é' })
        val emoji = "😀".repeat(80)
        val cut = WatchText.fit(emoji)
        assertTrue(cut.toByteArray(Charsets.UTF_8).size <= WatchText.MAX_BYTES)
        assertEquals(0, cut.length % 2)
        assertEquals("short", WatchText.fit("short"))
    }
}
