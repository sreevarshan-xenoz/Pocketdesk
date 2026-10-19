package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CameraAskTest {
    @Test
    fun aPhotoIsOfferedOnlyWithTheSwitchOnForTheOwnComputer() {
        assertTrue(CameraAsk.shouldOffer(enabled = true, fromOwnComputer = true))
        assertFalse(CameraAsk.shouldOffer(enabled = false, fromOwnComputer = true))
        assertFalse(CameraAsk.shouldOffer(enabled = true, fromOwnComputer = false))
    }

    @Test
    fun photosLeftOverAnHourAreCleared() {
        val now = 10 * 60 * 60_000L
        val files = listOf(
            "photo-old.jpg" to now - 2 * 60 * 60_000L,
            "photo-new.jpg" to now - 5 * 60_000L,
        )
        assertEquals(listOf("photo-old.jpg"), CameraAsk.stale(files, now))
    }
}
