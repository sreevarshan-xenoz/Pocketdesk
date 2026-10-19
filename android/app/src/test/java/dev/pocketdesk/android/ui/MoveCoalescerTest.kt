package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class MoveCoalescerTest {
    /** Runs a send when asked, after the caller returns, as a busy link does. */
    private class Later {
        val queued = ArrayDeque<() -> Unit>()
        fun launch(block: () -> Unit) = queued.addLast(block)
        fun runAll() {
            while (queued.isNotEmpty()) queued.removeFirst()()
        }
    }

    @Test
    fun movesAddUpWhileOneIsOnItsWay() {
        val later = Later()
        val sent = mutableListOf<String>()
        val moves = MoveCoalescer(later::launch, { x, y -> sent += "move $x $y" })
        moves.add(1, 2)
        moves.add(3, 4)
        later.runAll()
        assertEquals(listOf("move 4 6"), sent)
    }

    /** G4: letting go of the laser hides it after the moves asked before, never ahead of them. */
    @Test
    fun anEndGoesAfterTheMovesAskedBeforeIt() {
        val later = Later()
        val sent = mutableListOf<String>()
        val moves = MoveCoalescer(later::launch, { x, y -> sent += "move $x $y" }, { sent += "end" })
        moves.add(5, 0)
        moves.end()
        later.runAll()
        assertEquals(listOf("move 5 0", "end"), sent)
        moves.end()
        later.runAll()
        assertEquals(listOf("move 5 0", "end", "end"), sent)
        moves.add(1, 1)
        later.runAll()
        assertEquals("move 1 1", sent.last())
    }
}
