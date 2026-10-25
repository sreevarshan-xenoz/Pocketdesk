package dev.pocketdesk.android.status

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TickWakeTest {
    @Test
    fun `a poke ends the wait at once`() = runBlocking {
        val wake = TickWake()
        wake.poke()
        val start = System.nanoTime()
        assertTrue(wake.await(10_000))
        assertTrue((System.nanoTime() - start) / 1_000_000 < 1_000)
    }

    @Test
    fun `no poke waits the tick out`() = runBlocking {
        val wake = TickWake()
        val start = System.nanoTime()
        assertFalse(wake.await(50))
        assertTrue((System.nanoTime() - start) / 1_000_000 >= 50)
    }

    @Test
    fun `pokes while busy wake it once`() = runBlocking {
        val wake = TickWake()
        wake.poke()
        wake.poke()
        wake.poke()
        assertTrue(wake.await(10_000))
        assertFalse(wake.await(50))
    }
}
