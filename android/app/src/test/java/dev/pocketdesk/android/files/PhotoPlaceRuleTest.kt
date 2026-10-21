package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileNotFoundException

class PhotoPlaceRuleTest {
    private val q = 29
    private val r = 30
    private val route = PhotoPlaceRule.Route.ORIGINAL

    @Test
    fun `a MediaStore photo is asked for whole with the switch on and access granted`() {
        assertEquals(route, PhotoPlaceRule.route(on = true, granted = true, sdk = q, authority = "media"))
    }

    @Test
    fun `not without the switch or the access, nor before Android 10`() {
        val named = PhotoPlaceRule.Route.AS_NAMED
        assertEquals(named, PhotoPlaceRule.route(on = false, granted = true, sdk = q, authority = "media"))
        assertEquals(named, PhotoPlaceRule.route(on = true, granted = false, sdk = q, authority = "media"))
        assertEquals(named, PhotoPlaceRule.route(on = true, granted = true, sdk = 28, authority = "media"))
    }

    @Test
    fun `a picked file maps to its MediaStore item, external storage from Android 11`() {
        val mapped = PhotoPlaceRule.Route.MAPPED_ORIGINAL
        assertEquals(mapped, PhotoPlaceRule.route(on = true, granted = true, sdk = q, authority = PhotoPlaceRule.MEDIA_DOCUMENTS))
        assertEquals(mapped, PhotoPlaceRule.route(on = true, granted = true, sdk = r, authority = PhotoPlaceRule.EXTERNAL_STORAGE))
        assertEquals(PhotoPlaceRule.Route.AS_NAMED, PhotoPlaceRule.route(on = true, granted = true, sdk = q, authority = PhotoPlaceRule.EXTERNAL_STORAGE))
    }

    @Test
    fun `another provider's item decides for itself`() {
        val named = PhotoPlaceRule.Route.AS_NAMED
        assertEquals(named, PhotoPlaceRule.route(on = true, granted = true, sdk = r, authority = "com.google.android.apps.photos.contentprovider"))
        assertEquals(named, PhotoPlaceRule.route(on = true, granted = true, sdk = r, authority = null))
    }

    @Test
    fun `the original is opened first, and one Android refuses falls back to the item as named`() {
        val opened = PhotoPlaceRule.firstThatOpens("original", "plain") { "read $it" }
        assertEquals("read original", opened.value)
        assertTrue(opened.original)
        for (refusal in listOf(SecurityException(), UnsupportedOperationException(), FileNotFoundException(), IllegalArgumentException())) {
            val fell = PhotoPlaceRule.firstThatOpens("original", "plain") { if (it == "original") throw refusal else "read $it" }
            assertEquals("read plain", fell.value)
            assertFalse(fell.original)
        }
        val none = PhotoPlaceRule.firstThatOpens(null, "plain") { "read $it" }
        assertEquals("read plain", none.value)
        assertFalse(none.original)
    }

    @Test(expected = InterruptedException::class)
    fun `anything else is the send's own, not a fallback`() {
        PhotoPlaceRule.firstThatOpens("original", "plain") { throw InterruptedException() }
    }
}
