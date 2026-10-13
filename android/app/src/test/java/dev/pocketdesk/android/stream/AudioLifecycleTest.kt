package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Test

class AudioLifecycleTest {
    private var started = 0
    private var stopped = 0

    private fun lifecycle(enabled: Boolean = true) =
        AudioLifecycle(enabled) {
            started++
            Stoppable { stopped++ }
        }

    @Test
    fun aStreamPlaysFromUpUntilDown() {
        val audio = lifecycle()
        val gen = audio.newGeneration()
        audio.streamUp(gen)
        assertEquals(1, started)
        audio.streamDown(gen)
        assertEquals(1, stopped)
    }

    @Test
    fun anUpFromAStreamAlreadyStoppedStartsNothing() {
        val audio = lifecycle()
        val gen = audio.newGeneration()
        audio.streamDown(gen) // stop() won the race with the decode thread
        audio.streamUp(gen)
        assertEquals(0, started)
    }

    @Test
    fun aStaleDownDoesNotStopTheNextStream() {
        val audio = lifecycle()
        val first = audio.newGeneration()
        val second = audio.newGeneration()
        audio.streamUp(second)
        audio.streamDown(first)
        assertEquals(1, started)
        assertEquals(0, stopped)
    }

    @Test
    fun theSwitchStartsAndStopsALiveStreamsSound() {
        val audio = lifecycle(enabled = false)
        val gen = audio.newGeneration()
        audio.streamUp(gen)
        assertEquals(0, started)
        audio.setEnabled(true)
        assertEquals(1, started)
        audio.setEnabled(false)
        assertEquals(1, stopped)
        audio.streamDown(gen)
        assertEquals("nothing left to stop", 1, stopped)
    }
}
