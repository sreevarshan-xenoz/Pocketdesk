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
    fun namesTheComputerRefusesAreSkipped() {
        for (bad in listOf("NUL", "com1.txt", "a?b", "name.", "name ", "x\u0000y")) {
            assertFalse(bad, FolderWalk.sendable(bad))
        }
        for (fine in listOf("résumé.pdf", "COM10", "photo (1).jpg")) {
            assertTrue(fine, FolderWalk.sendable(fine))
        }
    }
}
