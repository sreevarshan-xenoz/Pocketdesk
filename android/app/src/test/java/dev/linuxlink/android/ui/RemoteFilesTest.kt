package dev.linuxlink.android.ui

import org.junit.Assert.assertEquals
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
    fun aFolderIsTitledByItsLastNameOnEitherSystem() {
        assertEquals("Documents", RemoteFiles.title("C:\\Users\\me\\Documents"))
        assertEquals("me", RemoteFiles.title("/home/me/"))
        assertEquals("", RemoteFiles.title(""))
    }

    @Test
    fun sizesReadNaturally() {
        assertEquals("512 B", RemoteFiles.humanSize(512))
        assertEquals("2.0 MB", RemoteFiles.humanSize(2_100_000))
        assertEquals("1.5 GB", RemoteFiles.humanSize(1_610_612_736))
    }
}
