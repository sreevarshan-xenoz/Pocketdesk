package dev.pocketdesk.android.pair

import org.junit.Assert.assertEquals
import org.junit.Test

class DisplayNameTest {
    @Test
    fun controlAndFormattingCharactersAreStrippedAndLongNamesCut() {
        // A right-to-left override could make a forged name read as another.
        assertEquals("Desk", PairLink.displayName("De\u202Esk\n"))
        assertEquals("a".repeat(32) + "…", PairLink.displayName("a".repeat(100)))
        assertEquals("", PairLink.displayName("\u0000\u200B"))
    }
}
