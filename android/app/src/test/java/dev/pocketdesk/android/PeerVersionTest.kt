package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Item 4: the computer card's release line, by the core's rules (`version.rs`). */
class PeerVersionTest {
    @Test
    fun releaseNumbersIgnoreSuffixes() {
        assertEquals(listOf(0L, 8L, 2L), PeerVersion.release("0.8.2"))
        assertEquals(listOf(0L, 8L, 2L), PeerVersion.release("v0.8.2"))
        assertEquals(listOf(0L, 8L, 2L), PeerVersion.release("0.8.2-dev"))
        assertEquals(listOf(0L, 8L, 2L), PeerVersion.release("0.8.2+g1a2b3c4"))
        assertEquals(listOf(0L, 9L, 0L), PeerVersion.release("0.9"))
        for (bad in listOf("", "dev", "-1.0", "1..2", "1.2.3.4", "1.x", "99999999999.0.0", "v")) {
            assertNull(bad, PeerVersion.release(bad))
        }
    }

    @Test
    fun releasesCompareByNumberNotByText() {
        assertEquals(-1, PeerVersion.compare("0.8.2", "0.8.1")!!.coerceIn(-1, 1))
        assertEquals(1, PeerVersion.compare("0.8.2", "0.10.0")!!.coerceIn(-1, 1))
        assertEquals(0, PeerVersion.compare("0.8.2", "0.8.2-dev"))
        assertNull(PeerVersion.compare("0.8.2", "garbage"))
    }

    @Test
    fun aClaimedVersionIsCleaned() {
        assertEquals("0.8.1", PeerVersion.clean(" 0.8.1 (built by hand)"))
        assertEquals("0.8.2-dev+g1a2b3c4", PeerVersion.clean("0.8.2-dev+g1a2b3c4"))
        assertNull(PeerVersion.clean(null))
        assertNull(PeerVersion.clean("latest"))
        assertNull(PeerVersion.clean("0.8.1<b>"))
        assertEquals(PeerVersion.MAX_CHARS, PeerVersion.clean("0.8.1-" + "a".repeat(80))!!.length)
    }

    @Test
    fun theCardSaysWhatToUpdate() {
        assertNull("a computer that says none", PeerVersion.line(null, "0.8.2", true))
        assertEquals(PeerVersion.Line(R.string.version_runs, "0.8.2", false), PeerVersion.line("0.8.2", "0.8.2", true))
        assertEquals(PeerVersion.Line(R.string.version_runs_older, "0.8.1", true), PeerVersion.line("0.8.1", "0.8.2", true))
        assertEquals(PeerVersion.Line(R.string.version_runs_newer, "0.9.0", true), PeerVersion.line("0.9.0", "0.8.2", true))
        assertEquals(
            "hints off: the release, plainly",
            PeerVersion.Line(R.string.version_runs, "0.8.1", false),
            PeerVersion.line("0.8.1", "0.8.2", false),
        )
        assertEquals(
            "this app's own release unknown: no advice",
            PeerVersion.Line(R.string.version_runs, "0.8.1", false),
            PeerVersion.line("0.8.1", "", true),
        )
    }
}
