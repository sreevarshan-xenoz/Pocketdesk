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
    fun aNameTheFolderHoldsGetsTheNextFree() {
        val taken = setOf("photo.jpg", "photo (1).jpg.part", "notes")
        assertEquals("photo (2).jpg", DownloadPlace.freeName("photo.jpg") { it in taken })
        assertEquals("notes (1)", DownloadPlace.freeName("notes") { it in taken })
        assertEquals("free.txt", DownloadPlace.freeName("free.txt") { it in taken })
        assertEquals("a dot first is no extension", ".bashrc (1)", DownloadPlace.freeName(".bashrc") { it == ".bashrc" })
    }

    @Test
    fun aDocumentIsToldFromAMediaStoreRow() {
        assertTrue(DownloadPlace.isDocument("content://com.android.externalstorage.documents/tree/primary%3AMovies/document/primary%3AMovies%2Fa.bin"))
        assertFalse(DownloadPlace.isDocument("content://media/external/downloads/146"))
        assertFalse(DownloadPlace.isDocument("/storage/emulated/0/Android/data/x/files/Download/a.bin"))
    }

    @Test
    fun aFileIsAPartUntilItIsWhole() {
        assertEquals("photo.jpg.part", DownloadPlace.partName("photo.jpg"))
        assertEquals("photo.jpg", DownloadPlace.finishedName("photo.jpg.part"))
        assertEquals("a provider's own name is kept", "photo (1).jpg", DownloadPlace.finishedName("photo (1).jpg"))
        assertEquals(".part", DownloadPlace.finishedName(".part"))
    }
}
