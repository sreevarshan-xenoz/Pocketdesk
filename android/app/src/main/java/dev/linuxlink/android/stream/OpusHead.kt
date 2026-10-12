package dev.linuxlink.android.stream

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * The codec-specific data Android's Opus decoder needs: `csd-0` is the
 * RFC 7845 identification header, and `csd-1`/`csd-2` are the pre-skip and
 * seek pre-roll as little-endian nanosecond counts. The desktop sends bare
 * Opus packets with no container, so the phone writes the header its
 * encoder would have. The wire format (48 kHz stereo) is fixed by the
 * server's audio pipeline.
 */
object OpusHead {
    const val SAMPLE_RATE = 48_000
    const val CHANNELS = 2

    /** libopus' look-ahead at 48 kHz, in samples (6.5 ms). */
    const val PRE_SKIP = 312

    /** RFC 7845's recommended seek pre-roll. */
    const val SEEK_PREROLL_NS = 80_000_000L

    fun csd0(): ByteArray =
        ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("OpusHead".toByteArray(Charsets.US_ASCII))
            put(1) // version
            put(CHANNELS.toByte())
            putShort(PRE_SKIP.toShort())
            putInt(SAMPLE_RATE) // the input rate; informational
            putShort(0) // output gain
            put(0) // channel mapping family 0: mono or stereo
        }.array()

    fun csd1(): ByteArray = nanos(PRE_SKIP * 1_000_000_000L / SAMPLE_RATE)

    fun csd2(): ByteArray = nanos(SEEK_PREROLL_NS)

    private fun nanos(value: Long): ByteArray =
        ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(value).array()
}
