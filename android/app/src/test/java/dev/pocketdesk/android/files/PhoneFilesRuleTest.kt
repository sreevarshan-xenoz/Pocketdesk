package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PhoneFilesRuleTest {
    @Test
    fun `a path is names that never climb`() {
        assertTrue(PhoneFilesRule.validPath(listOf("DCIM", "Camera")))
        assertTrue(PhoneFilesRule.validPath(emptyList()))
        for (bad in listOf("", ".", "..", "a/b", "a\\b", "line\nbreak")) {
            assertFalse(bad, PhoneFilesRule.validPath(listOf(bad)))
        }
        assertFalse(PhoneFilesRule.validPath(List(PhoneFilesRule.MAX_DEPTH + 1) { "a" }))
    }

    @Test
    fun `a folder's name is made one the computer takes`() {
        assertFalse(PhoneFilesRule.validName("Pics\u200DFamily"))
        assertEquals("Download", PhoneFilesRule.rootName("Download", "Folder 1"))
        assertEquals("PicsFamily", PhoneFilesRule.rootName("Pics\u200DFamily", "Folder 1"))
        assertEquals("Folder 1", PhoneFilesRule.rootName("..", "Folder 1"))
        assertEquals(255, PhoneFilesRule.rootName("a".repeat(300), "Folder 1").length)
    }

    @Test
    fun `hidden names are not listed`() {
        assertFalse(PhoneFilesRule.listed(".nomedia"))
        assertTrue(PhoneFilesRule.listed("photo.jpg"))
        assertFalse(PhoneFilesRule.listed("a/b"))
        assertTrue(PhoneFilesRule.reachable(listOf("Docs", "a.pdf")))
        assertFalse("never a hidden one, even named", PhoneFilesRule.reachable(listOf(".thumbnails", "a.jpg")))
    }

    @Test
    fun `requests come only from the phone's own computer`() {
        assertTrue(PhoneFilesRule.fromOwnComputer("192.0.2.7", 1716, "192.0.2.7", 1716))
        assertFalse(PhoneFilesRule.fromOwnComputer("192.0.2.7", 1717, "192.0.2.7", 1716))
        assertFalse(PhoneFilesRule.fromOwnComputer("198.51.100.3", 1716, "192.0.2.7", 1716))
        assertFalse(PhoneFilesRule.fromOwnComputer("192.0.2.7", 1716, null, null))
    }

    @Test
    fun `a root is found by its id and new roots take the next id`() {
        val folders = listOf(PhoneFolders.Folder(0, "content://a", "Download"), PhoneFolders.Folder(3, "content://b", "DCIM"))
        assertEquals("DCIM", PhoneFilesRule.root(folders, 3)?.name)
        assertNull(PhoneFilesRule.root(folders, 1))
        assertEquals(4, PhoneFilesRule.nextId(folders, 0))
        assertEquals(0, PhoneFilesRule.nextId(emptyList(), 0))
        // The last folder removed: its id is not handed out again.
        assertEquals(4, PhoneFilesRule.nextId(listOf(folders[0]), 4))
    }

    @Test
    fun `a drive fetch's id is 32 lowercase hex digits`() {
        assertEquals(true, PhoneFilesRule.validDrive("0123456789abcdef0123456789abcdef"))
        for (bad in listOf("", "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef", "../../0123456789abcdef0123456789", "g".repeat(32))) {
            assertEquals(bad, false, PhoneFilesRule.validDrive(bad))
        }
    }
}
