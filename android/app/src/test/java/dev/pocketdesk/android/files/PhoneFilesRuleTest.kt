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
    fun `hidden names are not listed`() {
        assertFalse(PhoneFilesRule.listed(".nomedia"))
        assertTrue(PhoneFilesRule.listed("photo.jpg"))
        assertFalse(PhoneFilesRule.listed("a/b"))
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
        assertEquals(4, PhoneFilesRule.nextId(folders))
        assertEquals(0, PhoneFilesRule.nextId(emptyList()))
    }
}
