package dev.pocketdesk.android.files

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenshotRuleTest {
    @Test
    fun `only images in a screenshots folder are screenshots`() {
        assertTrue(ScreenshotRule.inScreenshots("DCIM/Screenshots/"))
        assertTrue(ScreenshotRule.inScreenshots("Pictures/Screenshots/"))
        assertTrue(ScreenshotRule.inScreenshots("/storage/emulated/0/Pictures/screenshots"))
        assertFalse(ScreenshotRule.inScreenshots("DCIM/Camera/"))
        assertFalse(ScreenshotRule.inScreenshots("Pictures/ScreenshotsOld/"))
        assertFalse(ScreenshotRule.inScreenshots(null))
    }

    @Test
    fun `a screenshot is sent when new and recent`() {
        assertTrue(ScreenshotRule.due(addedSecs = 1_000, sinceSecs = 900, nowSecs = 1_005))
        assertFalse("taken before the switch was on", ScreenshotRule.due(899, 900, 905))
        assertFalse("too old when seen", ScreenshotRule.due(1_000, 900, 1_000 + ScreenshotRule.MAX_AGE_SECS + 1))
    }

    @Test
    fun `an empty or huge file is not sent`() {
        assertTrue(ScreenshotRule.sendable(2_000_000))
        assertFalse(ScreenshotRule.sendable(0))
        assertFalse(ScreenshotRule.sendable(ScreenshotRule.MAX_BYTES + 1))
    }
}
