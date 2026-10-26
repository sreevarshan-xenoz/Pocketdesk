package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Test

class TextQueueTest {
    private val saved = setOf("pc-a", "pc-b")

    /**
     * BC4: texts queued while the own computer was away went to whichever
     * computer was the own one when they left: one removed meanwhile handed
     * them to the next. Each goes to the computer it was kept for.
     */
    @Test
    fun `a text goes to the computer it was kept for`() {
        val fors = listOf("pc-a", "pc-b", "pc-a")
        assertEquals(TextQueue.Split(send = listOf(0, 2), drop = emptyList()), TextQueue.split(fors, "pc-a", saved))
        assertEquals(TextQueue.Split(send = listOf(1), drop = emptyList()), TextQueue.split(fors, "pc-b", saved))
    }

    @Test
    fun `a text kept for a computer since removed is dropped`() {
        val fors = listOf("pc-gone", "pc-b")
        assertEquals(TextQueue.Split(send = listOf(1), drop = listOf(0)), TextQueue.split(fors, "pc-b", saved))
        assertEquals("dropped with no own computer too", TextQueue.Split(emptyList(), listOf(0)), TextQueue.split(fors, null, saved))
    }

    @Test
    fun `a text kept with no computer paired goes to the first own one`() {
        assertEquals(TextQueue.Split(send = listOf(0), drop = emptyList()), TextQueue.split(listOf(""), "pc-a", saved))
        assertEquals(TextQueue.Split(emptyList(), emptyList()), TextQueue.split(listOf(""), null, saved))
    }

    @Test
    fun `at most a batch goes at once`() {
        val fors = List(60) { "pc-a" }
        assertEquals(TextQueue.BATCH, TextQueue.split(fors, "pc-a", saved).send.size)
    }
}
