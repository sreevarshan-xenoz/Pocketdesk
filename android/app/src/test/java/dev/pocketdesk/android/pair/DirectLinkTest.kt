package dev.pocketdesk.android.pair

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DirectLinkTest {
    private val wifi = WifiJoin("pocketdesk-ab12", "Abcdef123456")

    @Test
    fun anOrdinaryLinkJoinsNothing() {
        for (enabled in listOf(true, false)) {
            assertEquals(
                DirectLinkDecision.NotADirectLink,
                DirectLinkDecision.decide(null, enabled, 34),
            )
        }
    }

    @Test
    fun aDirectLinkIsJoinedWhenTheSwitchIsOnAndAndroidIsNewEnough() {
        assertEquals(DirectLinkDecision.Join, DirectLinkDecision.decide(wifi, true, 29))
        assertEquals(DirectLinkDecision.Join, DirectLinkDecision.decide(wifi, true, 37))
    }

    @Test
    fun theSwitchOffRefusesBeforeAnythingIsJoined() {
        assertEquals(DirectLinkDecision.Disabled, DirectLinkDecision.decide(wifi, false, 34))
        // Off wins over an old phone: the owner's word is what is said.
        assertEquals(DirectLinkDecision.Disabled, DirectLinkDecision.decide(wifi, false, 26))
    }

    @Test
    fun androidNineAndOlderCannotJoin() {
        assertEquals(DirectLinkDecision.TooOld, DirectLinkDecision.decide(wifi, true, 28))
        assertEquals(DirectLinkDecision.TooOld, DirectLinkDecision.decide(wifi, true, 26))
    }

    @Test
    fun wifiDetailsAreCheckedLikeCoresDo() {
        assertTrue(WifiJoin("x", "12345678").isValid())
        assertFalse(WifiJoin("", "12345678").isValid())
        assertFalse(WifiJoin("n".repeat(33), "12345678").isValid())
        assertFalse(WifiJoin("a\nb", "12345678").isValid())
        assertFalse(WifiJoin("x", "1234567").isValid())
        assertFalse(WifiJoin("x", "k".repeat(64)).isValid())
        assertFalse(WifiJoin("x", "p\u00e4ssword1").isValid())
        // A name is 32 bytes, not 32 characters.
        assertFalse(WifiJoin("\u00e9".repeat(17), "12345678").isValid())
        assertTrue(WifiJoin("\u00e9".repeat(16), "12345678").isValid())
    }
}
