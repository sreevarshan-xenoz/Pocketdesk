package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteFilesTest {
    private fun entry(name: String, dir: Boolean) = RemoteEntry(name, "/home/me/$name", dir, 0, 0)

    @Test
    fun foldersComeFirstThenNamesRegardlessOfCase() {
        val sorted = RemoteFiles.sorted(
            listOf(entry("b.txt", false), entry("Zeta", true), entry("A.txt", false), entry("alpha", true)),
        )
        assertEquals(listOf("alpha", "Zeta", "A.txt", "b.txt"), sorted.map { it.name })
    }

    @Test
    fun aFolderBelowHomeCanBeDownloadedWholeButHomeCannot() {
        val docs = RemoteFolder("/home/me/Docs", parent = "/home/me", entries = emptyList(), error = null)
        assertTrue(RemoteFiles.canDownloadWhole(docs))
        assertFalse("home holds too much", RemoteFiles.canDownloadWhole(docs.copy(parent = null)))
        assertFalse("a folder that did not load", RemoteFiles.canDownloadWhole(docs.copy(error = "denied")))
    }

    @Test
    fun aFolderIsTitledByItsLastNameOnEitherSystem() {
        assertEquals("Documents", RemoteFiles.title("C:\\Users\\me\\Documents"))
        assertEquals("me", RemoteFiles.title("/home/me/"))
        assertEquals("", RemoteFiles.title(""))
    }

    @Test
    fun aFolderThatFailedToLoadStillHasAWayBack() {
        val docs = RemoteFolder("/home/me/docs", "/home/me", emptyList(), null)
        assertEquals("back to where it came from", "/home/me/docs", RemoteFiles.wayBack("/home/me/docs/x", docs))
        assertEquals("a reload that failed goes up", "/home/me", RemoteFiles.wayBack("/home/me/docs", docs))
        assertEquals("nothing loaded yet", null, RemoteFiles.wayBack("", null))
    }

    @Test
    fun sizesReadNaturally() {
        assertEquals("512 B", RemoteFiles.humanSize(512))
        assertEquals("2.0 MB", RemoteFiles.humanSize(2_100_000))
        assertEquals("1.5 GB", RemoteFiles.humanSize(1_610_612_736))
    }
}
