package dev.pocketdesk.android.doctor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BackgroundRunningTest {
    @Test
    fun `the card shows only when a background feature is limited and the reminder is on`() {
        assertTrue(BackgroundRunning.shouldRemind(true, BackgroundLimit.OPTIMIZED, true))
        assertTrue(BackgroundRunning.shouldRemind(true, BackgroundLimit.RESTRICTED, true))
        assertFalse(BackgroundRunning.shouldRemind(true, BackgroundLimit.NONE, true))
        assertFalse(BackgroundRunning.shouldRemind(false, BackgroundLimit.OPTIMIZED, true))
        assertFalse(BackgroundRunning.shouldRemind(true, BackgroundLimit.OPTIMIZED, false))
        // V2: not beside the health card saying the same.
        assertFalse(BackgroundRunning.shouldRemind(true, BackgroundLimit.OPTIMIZED, true, healthSaysPaused = true))
    }

    @Test
    fun `makers' own battery managers get their guide`() {
        assertEquals("https://dontkillmyapp.com/samsung", BackgroundRunning.makerGuide("samsung"))
        assertEquals("https://dontkillmyapp.com/oppo", BackgroundRunning.makerGuide("OPPO"))
        assertEquals("https://dontkillmyapp.com/xiaomi", BackgroundRunning.makerGuide("Redmi"))
        assertEquals("https://dontkillmyapp.com/huawei", BackgroundRunning.makerGuide("HONOR"))
        assertNull(BackgroundRunning.makerGuide("Fairphone Unknown"))
    }
}
