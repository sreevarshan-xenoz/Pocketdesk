package dev.linuxlink.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadNameTest {
    @Test
    fun aNameFromTheComputerNeverNamesAFolder() {
        assertEquals("report.pdf", DownloadName.safe("report.pdf"))
        assertEquals("evil.sh", DownloadName.safe("../../evil.sh"))
        assertEquals("x.txt", DownloadName.safe("C:\\Users\\me\\x.txt"))
        assertEquals("ab.txt", DownloadName.safe("a\u0000b.txt"))
        assertEquals("bashrc", DownloadName.safe(".bashrc"))
        assertEquals("résumé 2026.pdf", DownloadName.safe("résumé 2026.pdf"))
    }

    @Test
    fun anEmptyOrDotNameGetsAPlaceholder() {
        assertEquals("download", DownloadName.safe(""))
        assertEquals("download", DownloadName.safe(".."))
        assertEquals("download", DownloadName.safe("   "))
    }

    @Test
    fun aLongNameKeepsItsExtension() {
        val fitted = DownloadName.safe("a".repeat(300) + ".pdf")
        assertTrue(fitted, fitted.length <= DownloadName.MAX_LENGTH && fitted.endsWith(".pdf"))
    }

    @Test
    fun theTypeComesFromTheExtension() {
        assertEquals("pdf", DownloadName.extension("Report.PDF"))
        assertEquals("", DownloadName.extension("README"))
        assertEquals("gz", DownloadName.extension("a.tar.gz"))
    }
}
