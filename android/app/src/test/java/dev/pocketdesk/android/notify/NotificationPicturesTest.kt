package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationPicturesTest {
    /** K3: an app's icon goes with its first notification only. */
    @Test
    fun `an icon goes once per app`() {
        assertTrue(NotificationPictures.sendIcon(emptySet(), "com.chat"))
        assertFalse(NotificationPictures.sendIcon(setOf("com.chat"), "com.chat"))
        assertTrue(NotificationPictures.sendIcon(setOf("com.chat"), "com.mail"))
    }

    /** K3: a picture goes only with the setting on and the notification in full. */
    @Test
    fun `a picture goes only in full with the setting on`() {
        assertTrue(NotificationPictures.sendPicture(enabled = true, full = true))
        assertFalse("redacted", NotificationPictures.sendPicture(enabled = true, full = false))
        assertFalse("off", NotificationPictures.sendPicture(enabled = false, full = true))
    }

    /** K3: pictures fit their long side, never grow, and keep their shape. */
    @Test
    fun `a picture fits its long side`() {
        assertEquals(720 to 405, NotificationPictures.fit(1920, 1080, 720))
        assertEquals(405 to 720, NotificationPictures.fit(1080, 1920, 720))
        assertEquals(48 to 48, NotificationPictures.fit(48, 48, 64))
        assertEquals(1 to 1, NotificationPictures.fit(0, 10, 64))
        assertEquals(64 to 1, NotificationPictures.fit(4000, 10, 64))
    }
}
