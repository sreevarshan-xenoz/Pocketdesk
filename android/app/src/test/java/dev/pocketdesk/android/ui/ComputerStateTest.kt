package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ComputerStateTest {
    @Test
    fun aSpaceIsABarAndItsWords() {
        val memory = ComputerState.space("", 8L * 1024 * 1024 * 1024, 16L * 1024 * 1024 * 1024)!!
        assertEquals(0.5f, memory.fraction, 1e-6f)
        assertEquals("8.0 GB of 16.0 GB", memory.words("of"))
        // More used than there is reads as full, not past it.
        assertEquals(1f, ComputerState.space("C:", 20, 10)!!.fraction, 1e-6f)
        assertNull(ComputerState.space("D:", 1, 0))
        assertNull(ComputerState.space("E:", -1, 10))
    }

    @Test
    fun uptimeIsSaidInItsTwoLargestUnits() {
        assertEquals("3 d 4 h", ComputerState.uptime(3 * 86_400L + 4 * 3_600 + 59, "d", "h", "min"))
        assertEquals("5 h 12 min", ComputerState.uptime(5 * 3_600L + 12 * 60 + 30, "d", "h", "min"))
        assertEquals("7 min", ComputerState.uptime(7 * 60L + 5, "d", "h", "min"))
        assertEquals("0 min", ComputerState.uptime(20, "d", "h", "min"))
    }
}
