package dev.linuxlink.android.bridge

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioPayloadTest {
    /** android/bridge/src/api.rs's AUDIO_FIXTURE_HEX; both tests pin it. */
    private val fixture = hex("00000002000000000000000700000003010203000000000000000800000000")

    private fun hex(s: String) = ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    @Test
    fun parsesTheBridgesLayout() {
        val chunks = AudioPayload.parse(fixture)
        assertEquals(listOf(7L, 8L), chunks.map { it.sequence })
        assertArrayEquals(byteArrayOf(1, 2, 3), chunks[0].data)
        assertEquals(0, chunks[1].data.size)
    }

    @Test
    fun aTruncatedPayloadYieldsTheWholePacketsOnly() {
        assertEquals(listOf(7L), AudioPayload.parse(fixture.copyOf(fixture.size - 3)).map { it.sequence })
        assertTrue(AudioPayload.parse(ByteArray(0)).isEmpty())
        // A count far beyond the bytes, and a negative length.
        assertTrue(AudioPayload.parse(hex("7fffffff")).isEmpty())
        assertTrue(AudioPayload.parse(hex("00000001" + "0000000000000001" + "ffffffff")).isEmpty())
    }
}
