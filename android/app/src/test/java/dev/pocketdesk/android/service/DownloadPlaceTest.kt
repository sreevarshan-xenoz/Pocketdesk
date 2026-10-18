package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadPlaceTest {
    private val tree = "content://com.android.externalstorage.documents/tree/primary%3AMovies"

    @Test
    fun aPickedFolderIsUsedWhileItsGrantHolds() {
        assertEquals(tree, DownloadPlace.choose(tree, listOf(tree)))
        assertNull("its grant was taken back", DownloadPlace.choose(tree, emptyList()))
        assertNull("none picked", DownloadPlace.choose(null, listOf(tree)))
        assertNull(DownloadPlace.choose("", listOf("")))
    }

    @Test
    fun aFolderPickedAndNoLongerWritableIsSaid() {
        assertTrue(DownloadPlace.lost(tree, emptyList()))
        assertFalse(DownloadPlace.lost(tree, listOf(tree)))
        assertFalse("none picked is not lost", DownloadPlace.lost(null, emptyList()))
    }

    @Test
    fun aFileIsAPartUntilItIsWhole() {
        assertEquals("photo.jpg.part", DownloadPlace.partName("photo.jpg"))
        assertEquals("photo.jpg", DownloadPlace.finishedName("photo.jpg.part"))
        assertEquals("a provider's own name is kept", "photo (1).jpg", DownloadPlace.finishedName("photo (1).jpg"))
        assertEquals(".part", DownloadPlace.finishedName(".part"))
    }
}
