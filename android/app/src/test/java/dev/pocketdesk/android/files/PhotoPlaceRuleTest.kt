package dev.pocketdesk.android.files

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PhotoPlaceRuleTest {
    private val q = 29

    @Test
    fun `a MediaStore photo is asked for whole with the switch on and the permission`() {
        assertTrue(PhotoPlaceRule.asksOriginal(on = true, granted = true, sdk = q, authority = "media"))
    }

    @Test
    fun `not without the switch or the permission`() {
        assertFalse(PhotoPlaceRule.asksOriginal(on = false, granted = true, sdk = q, authority = "media"))
        assertFalse(PhotoPlaceRule.asksOriginal(on = true, granted = false, sdk = q, authority = "media"))
    }

    @Test
    fun `not before Android 10, which takes nothing out`() {
        assertFalse(PhotoPlaceRule.asksOriginal(on = true, granted = true, sdk = 28, authority = "media"))
    }

    @Test
    fun `not another provider's item, which decides for itself`() {
        assertFalse(PhotoPlaceRule.asksOriginal(on = true, granted = true, sdk = q, authority = "com.android.externalstorage.documents"))
        assertFalse(PhotoPlaceRule.asksOriginal(on = true, granted = true, sdk = q, authority = "com.google.android.apps.photos.contentprovider"))
        assertFalse(PhotoPlaceRule.asksOriginal(on = true, granted = true, sdk = q, authority = null))
    }
}
