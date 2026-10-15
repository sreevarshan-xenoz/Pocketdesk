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

    /** The same cases as `core::protocol::link`'s loopback test; keep them alike. */
    @Test
    fun aLinkToALoopbackHostOrNoHostIsRefused() {
        for (bad in listOf(
            "https://:80/",
            "http://localhost:8080/admin",
            "http://LOCALHOST./",
            "http://app.localhost/",
            "http://127.0.0.1/",
            "http://127.9.9.9:3000/",
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0x7f.0.0.1/",
            "http://0177.0.0.1/",
            "http://0.0.0.0:8000/",
            "http://[::1]:8000/",
            "http://[::]/",
            "http://[::ffff:127.0.0.1]/",
            "http://%6c%6fcalhost/",
            "http://\uff11\uff12\uff17.0.0.1/",
            "http://127\u30020\u30020\u30021/",
        )) {
            assertFalse(bad, LinkRule.openable(bad))
        }
        for (fine in listOf(
            "http://192.0.2.7/",
            "http://[2001:db8::1]:8080/",
            "https://localhost.example.org/",
            "https://cafe.de/",
            "https://1password.com/",
            "https://m\u00fcnchen.de/",
        )) {
            assertTrue(fine, LinkRule.openable(fine))
        }
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

    @Test
    fun punctuationAroundALinkInASentenceIsNotPartOfIt() {
        assertEquals("https://example.org/p", LinkRule.find("See https://example.org/p."))
        assertEquals("https://example.org/p", LinkRule.find("(https://example.org/p)"))
        assertEquals("https://example.org/p", LinkRule.find("\"https://example.org/p\","))
        assertEquals("https://example.org/p", LinkRule.find("<https://example.org/p>"))
        // A bracket the link opened is its own.
        assertEquals(
            "https://en.wikipedia.org/wiki/Foo_(bar)",
            LinkRule.find("Read https://en.wikipedia.org/wiki/Foo_(bar)!"),
        )
    }
}
