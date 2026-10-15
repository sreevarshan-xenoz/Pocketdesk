package dev.pocketdesk.android.links

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LinkRuleTest {
    @Test
    fun onlyAPlainWebLinkMayBeOpenedTheSameRulesAsTheComputer() {
        for (fine in listOf("https://example.org/a?b=c#d", "http://192.0.2.7:8080/", "HTTPS://Example.org")) {
            assertTrue(fine, LinkRule.openable(fine))
        }
        for (bad in listOf(
            "file:///sdcard/x", "javascript:alert(1)", "intent://x#Intent;end",
            "https://user:pass@example.org/", "https://", "https://example.org/a b", "",
        )) {
            assertFalse(bad, LinkRule.openable(bad))
        }
        assertFalse(LinkRule.openable("https://example.org/" + "a".repeat(LinkRule.MAX_BYTES)))
    }

    @Test
    fun theLinkIsFoundInWhatABrowserShares() {
        // Browsers often share "title + link".
        assertEquals(
            "https://example.org/story",
            LinkRule.find("A good story https://example.org/story"),
        )
        assertEquals("https://example.org/", LinkRule.find("https://example.org/"))
        assertNull(LinkRule.find("no link here"))
        assertNull(LinkRule.find("javascript:alert(1)"))
    }
}
