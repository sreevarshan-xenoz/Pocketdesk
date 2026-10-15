package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FolderWalkTest {
    private fun dir(id: String, name: String) = FolderWalk.Entry(id, name, isDir = true, size = 0)
    private fun file(id: String, name: String, size: Long = 10) = FolderWalk.Entry(id, name, isDir = false, size = size)

    private val tree = mapOf(
        "root" to listOf(dir("d1", "2026"), file("f1", "top.jpg"), file("h", ".nomedia"), dir("hd", ".thumbnails")),
        "d1" to listOf(file("f2", "beach.jpg"), file("bad", "a:b.txt"), dir("d2", "deep")),
        "d2" to listOf(file("f3", "x.txt")),
        "hd" to listOf(file("t", "thumb.jpg")),
    )

    @Test
    fun filesComeWithTheirPathAndHiddenOnesAreSkipped() {
        val plan = FolderWalk.plan("root") { tree[it].orEmpty() }
        assertEquals(
            // Folders first, then files, each by name: the order the
            // computer's own browse shows.
            listOf(
                listOf("2026", "deep", "x.txt"),
                listOf("2026", "beach.jpg"),
                listOf("top.jpg"),
            ),
            plan.files.map { it.path },
        )
        assertEquals(listOf("f3", "f2", "f1"), plan.files.map { it.id })
        assertEquals("one name the computer would refuse", 1, plan.skipped)
        assertFalse(plan.truncated)
        assertEquals(30L, plan.totalBytes)
    }

    @Test
    fun aFolderThatCannotBeListedIsCountedNotTakenForEmpty() {
        // The provider fails to list "2026": its files are left out, and said so.
        val plan = FolderWalk.plan("root") { if (it == "d1") null else tree[it].orEmpty() }
        assertEquals(listOf(listOf("top.jpg")), plan.files.map { it.path })
        assertEquals(1, plan.skipped)
        // A root that cannot be listed leaves nothing, counted.
        val none = FolderWalk.plan("root") { null }
        assertTrue(none.files.isEmpty())
        assertEquals(1, none.skipped)
    }

    @Test
    fun aFolderTooBigIsCutShortAndSaysSo() {
        val many = (1..FolderWalk.MAX_FILES + 5).map { file("f$it", "f$it.bin") }
        val plan = FolderWalk.plan("root") { if (it == "root") many else emptyList() }
        assertEquals(FolderWalk.MAX_FILES, plan.files.size)
        assertTrue(plan.truncated)
    }

    @Test
    fun aFolderPastItsDepthIsNotWalkedFurther() {
        val chain = { id: String ->
            val n = id.removePrefix("d").toIntOrNull() ?: 0
            listOf(dir("d${n + 1}", "level${n + 1}"), file("f$n", "file$n.txt"))
        }
        val plan = FolderWalk.plan("d0", chain)
        assertTrue(plan.files.all { it.path.size <= FolderWalk.MAX_DEPTH })
        assertTrue(plan.truncated)
    }

    @Test
    fun aReceivedFolderGetsANameNoEarlierOneHas() {
        assertEquals("Trip", FolderWalk.freeName("Trip") { false })
        val taken = setOf("Trip", "Trip (1)")
        assertEquals("Trip (2)", FolderWalk.freeName("Trip") { it in taken })
    }

    @Test
    fun aPlaceFromTheComputerIsTakenToTheSameDepthTheComputerAllows() {
        val deepest = (1..FolderWalk.MAX_DEPTH).map { "d$it" }
        assertTrue("as deep as the computer sends", FolderWalk.receivable("Trip", deepest))
        assertFalse(FolderWalk.receivable("Trip", deepest + "x"))
        assertFalse("no name", FolderWalk.receivable("Trip", emptyList()))
        assertFalse(FolderWalk.receivable("..", listOf("a.jpg")))
        assertFalse(FolderWalk.receivable("Trip", listOf("..", "a.jpg")))
    }

    @Test
    fun namesTheComputerRefusesAreSkipped() {
        for (bad in listOf(
            "NUL", "com1.txt", "COM0", "lpt0.log", "a?b", "name.", "name ", "x\u0000y",
            // Invisible characters: a bidi override, a zero-width space.
            "photo\u202Egpj.exe", "b\u200B.txt",
        )) {
            assertFalse(bad, FolderWalk.sendable(bad))
        }
        for (fine in listOf("résumé.pdf", "COM10", "photo (1).jpg")) {
            assertTrue(fine, FolderWalk.sendable(fine))
        }
    }
}
