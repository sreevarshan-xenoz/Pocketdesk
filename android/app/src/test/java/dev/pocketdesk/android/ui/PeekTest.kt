package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class PeekTest {
    @Test
    fun theComputersReasonsAreKnown() {
        assertEquals(Peek.Refusal.Off, Peek.refusal("off"))
        assertEquals(Peek.Refusal.Paused, Peek.refusal("paused"))
        assertEquals(Peek.Refusal.Busy, Peek.refusal("busy"))
        assertEquals(Peek.Refusal.TooSoon, Peek.refusal("too_soon"))
        assertEquals(Peek.Refusal.NoScreen, Peek.refusal("no_screen"))
        assertEquals(Peek.Refusal.NotAllowed, Peek.refusal("not_allowed"))
    }

    @Test
    fun onlyAStillOfAScreensSizeIsDecoded() {
        assertEquals(true, Peek.fits(1600, 900))
        assertEquals(true, Peek.fits(720, 1600))
        assertEquals(false, Peek.fits(0, 900))
        assertEquals(false, Peek.fits(30000, 30000))
        assertEquals(false, Peek.fits(1600, -1))
    }

    @Test
    fun onlyStillsAreLeftoversInTheCache() {
        val dir = kotlin.io.path.createTempDirectory("peek-test").toFile()
        try {
            val still = java.io.File(dir, "peek123.jpg").apply { writeText("x") }
            java.io.File(dir, "photo.jpg").writeText("x")
            java.io.File(dir, "peek.txt").writeText("x")
            java.io.File(dir, "peek-folder.jpg").mkdir()
            assertEquals(listOf(still), Peek.leftovers(dir.listFiles()!!.toList()))
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun anythingElseIsShownAsItCame() {
        assertEquals(Peek.Refusal.Other, Peek.refusal(null))
        assertEquals(Peek.Refusal.Other, Peek.refusal("Connection refused"))
        assertEquals(
            Peek.Refusal.Other,
            Peek.refusal("The computer refused the download: no free transfer slot"),
        )
    }
}
