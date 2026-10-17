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
}
