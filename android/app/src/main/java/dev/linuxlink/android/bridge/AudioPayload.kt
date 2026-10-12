package dev.linuxlink.android.bridge

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** One Opus packet from the desktop, with the sequence it was sent under. */
class AudioChunk(val sequence: Long, val data: ByteArray)

/**
 * The bridge's audio payload (`api::audio_payload` in the Rust bridge),
 * big-endian: `u32 count`, then per packet `u64 sequence`, `u32 len`,
 * `len` bytes. A short or malformed payload yields the whole packets
 * before the fault; it never throws.
 */
object AudioPayload {
    fun parse(bytes: ByteArray): List<AudioChunk> {
        val buf = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
        if (buf.remaining() < 4) return emptyList()
        val count = buf.int
        val chunks = ArrayList<AudioChunk>(count.coerceIn(0, 32))
        repeat(count.coerceAtLeast(0)) {
            if (buf.remaining() < 12) return chunks
            val sequence = buf.long
            val len = buf.int
            if (len < 0 || buf.remaining() < len) return chunks
            val data = ByteArray(len)
            buf.get(data)
            chunks += AudioChunk(sequence, data)
        }
        return chunks
    }
}
