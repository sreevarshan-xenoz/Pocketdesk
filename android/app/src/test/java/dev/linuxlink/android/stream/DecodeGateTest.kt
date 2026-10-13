package dev.linuxlink.android.stream

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DecodeGateTest {
    @Test
    fun aDecoderStoppedBeforeItStartsNeverRuns() {
        // A screen switch stops the stream while its connect is still in
        // flight; the decode loop that starts afterwards must not run.
        val gate = DecodeGate()
        gate.stop()
        assertFalse(gate.open())
        assertFalse(gate.isOpen)
    }

    @Test
    fun aRunningDecoderStopsAndStaysStopped() {
        val gate = DecodeGate()
        assertTrue(gate.open())
        assertTrue(gate.isOpen)
        gate.stop()
        assertFalse(gate.isOpen)
        assertFalse("not reopened", gate.open())
    }
}
