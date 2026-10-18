package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ChromeTextTest {
    @Test
    fun theChromesTextFollowsTheOwnersScaleUpTo1point3() {
        assertEquals(0.85f, ChromeText.scale(0.85f), 0f)
        assertEquals(1f, ChromeText.scale(1f), 0f)
        assertEquals(1.3f, ChromeText.scale(1.3f), 0f)
        assertEquals(1.3f, ChromeText.scale(2f), 0f)
    }
}
