package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class UploadMarksTest {
    /** BB4: the mark moves past what arrived, never past one that did not. */
    @Test
    fun `the mark moves only past what arrived`() {
        val went = mapOf(11L to true, 12L to true, 13L to false, 14L to true)
        assertEquals(12L, UploadMarks.after(listOf(11, 12, 13, 14)) { went[it] })
        assertNull("the first failed: the mark stays", UploadMarks.after(listOf(13, 14)) { went[it] })
        assertNull("not known yet: it goes again", UploadMarks.after(listOf(20)) { null })
        assertEquals(14L, UploadMarks.after(listOf(14)) { went[it] })
        assertNull(UploadMarks.after(emptyList()) { true })
    }
}
