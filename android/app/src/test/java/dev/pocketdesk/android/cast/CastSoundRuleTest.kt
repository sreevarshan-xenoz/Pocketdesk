package dev.pocketdesk.android.cast

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CastSoundRuleTest {
    /** K5: the sound goes only with everything in place. */
    @Test
    fun `the sound goes with the setting, the permission and the computer`() {
        assertTrue(CastSoundRule.capture(true, true, true, false, 34))
        assertFalse("setting off", CastSoundRule.capture(false, true, true, false, 34))
        assertFalse("no permission", CastSoundRule.capture(true, false, true, false, 34))
        assertFalse("the computer drops it", CastSoundRule.capture(true, true, false, false, 34))
        assertFalse("the camera", CastSoundRule.capture(true, true, true, true, 34))
        assertFalse("Android 9", CastSoundRule.capture(true, true, true, false, 28))
        assertTrue("Android 10", CastSoundRule.capture(true, true, true, false, 29))
    }
}
