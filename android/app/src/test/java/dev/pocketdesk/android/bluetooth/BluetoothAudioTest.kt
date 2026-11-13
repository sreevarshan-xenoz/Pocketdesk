package dev.pocketdesk.android.bluetooth

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BluetoothAudioTest {
    @Test
    fun audio_busy_when_playing_or_in_call() {
        assertFalse(BluetoothAudio.busy(a2dpPlaying = false, inCall = false))
        assertTrue(BluetoothAudio.busy(a2dpPlaying = true, inCall = false))
        assertTrue(BluetoothAudio.busy(a2dpPlaying = false, inCall = true))
        assertTrue(BluetoothAudio.busy(a2dpPlaying = true, inCall = true))
    }
}
