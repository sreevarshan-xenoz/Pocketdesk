package dev.pocketdesk.android.compat

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class Android17Test {
    /** From 17 a volume set from the background is ignored, and put back may never be. */
    @Test
    fun theRingRaisesTheAlarmVolumeOnlyBefore17() {
        assertTrue(Android17.ringRaisesVolume(36))
        assertFalse(Android17.ringRaisesVolume(37))
        assertFalse(Android17.ringRaisesVolume(38))
    }

    /** A touchpad sent absolute finger positions under capture before 17. */
    @Test
    fun aTouchpadIsCapturedFrom17() {
        assertTrue(Android17.captures(sdk = 36, virtual = false, mouse = true, touchpad = false))
        assertFalse(Android17.captures(sdk = 36, virtual = false, mouse = true, touchpad = true))
        assertTrue(Android17.captures(sdk = 37, virtual = false, mouse = true, touchpad = true))
        assertTrue(Android17.captures(sdk = 37, virtual = false, mouse = false, touchpad = true))
        assertFalse(Android17.captures(sdk = 37, virtual = true, mouse = true, touchpad = true))
        assertFalse(Android17.captures(sdk = 37, virtual = false, mouse = false, touchpad = false))
    }
}
