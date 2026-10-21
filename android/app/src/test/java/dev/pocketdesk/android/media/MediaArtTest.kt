package dev.pocketdesk.android.media

import org.junit.Assert.assertEquals
import org.junit.Test

class MediaArtTest {
    /** N2: the id the computer checks a cover by, the first 16 hex digits of its SHA-256. */
    @Test
    fun aCoverIsNamedByItsBytes() {
        // SHA-256("abc") begins ba7816bf8f01cfea.
        assertEquals("ba7816bf8f01cfea", MediaArt.id("abc".toByteArray()))
    }

    @Test
    fun aCoverIsScaledToFitNeverUp() {
        assertEquals(256 to 128, MediaArt.fit(1024, 512))
        assertEquals(128 to 256, MediaArt.fit(300, 600))
        assertEquals(100 to 50, MediaArt.fit(100, 50))
        assertEquals(256 to 1, MediaArt.fit(5000, 1))
    }
}
