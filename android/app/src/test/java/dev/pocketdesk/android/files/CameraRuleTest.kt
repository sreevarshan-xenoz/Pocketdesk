package dev.pocketdesk.android.files

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CameraRuleTest {
    @Test
    fun `only the camera's folder is the camera's`() {
        assertTrue(CameraRule.inCamera("DCIM/Camera/"))
        assertTrue(CameraRule.inCamera("DCIM/camera/"))
        assertTrue(CameraRule.inCamera("DCIM/Camera/Burst/"))
        assertTrue("before Android 10, the file's folder", CameraRule.inCamera("/storage/emulated/0/DCIM/Camera"))
        assertFalse(CameraRule.inCamera("DCIM/Screenshots/"))
        assertFalse(CameraRule.inCamera("DCIM/ScreenRecorder/"))
        assertFalse(CameraRule.inCamera("Pictures/Camera/"))
        assertFalse(CameraRule.inCamera("DCIM/"))
        assertFalse(CameraRule.inCamera("DCIM/CameraOld/"))
        assertFalse(CameraRule.inCamera(null))
    }

    @Test
    fun `one taken since the switch was on goes, however long ago`() {
        assertTrue(CameraRule.due(addedSecs = 1_000, sinceSecs = 900))
        assertTrue(CameraRule.due(addedSecs = 900, sinceSecs = 900))
        assertFalse("taken before the switch was on", CameraRule.due(899, 900))
    }

    @Test
    fun `an empty one is waited for a while, then passed`() {
        assertTrue(CameraRule.mayStillBeWritten(addedSecs = 1_000, nowSecs = 1_010))
        assertFalse(CameraRule.mayStillBeWritten(addedSecs = 1_000, nowSecs = 1_000 + CameraRule.EMPTY_WAIT_SECS))
    }

    @Test
    fun `wifi only holds them back off wifi`() {
        assertTrue(CameraRule.mayGo(wifiOnly = true, onWifi = true))
        assertFalse(CameraRule.mayGo(wifiOnly = true, onWifi = false))
        assertTrue(CameraRule.mayGo(wifiOnly = false, onWifi = false))
    }
}
