package dev.pocketdesk.android.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PinRulesTest {
    @Test
    fun aPinReadOffTheDesktopCanBeEnteredWhileTheSheetWaits() {
        // "Show PIN on desktop" leaves the sheet waiting for the desktop;
        // the PIN it shows must still be enterable.
        assertTrue(PinRules.canSubmit("123456", success = false))
    }

    @Test
    fun onlySixDigitsBeforePairingSucceedsCanBeSubmitted() {
        assertFalse(PinRules.canSubmit("12345", success = false))
        assertFalse(PinRules.canSubmit("123456", success = true))
    }
}
