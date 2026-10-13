package dev.pocketdesk.android.stream

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class OpusHeadTest {
    @Test
    fun csd0IsAnRfc7845IdentificationHeader() {
        val head = OpusHead.csd0()
        assertEquals(19, head.size)
        assertEquals("OpusHead", String(head, 0, 8, Charsets.US_ASCII))
        assertArrayEquals(
            // version 1, 2 channels, pre-skip 312 LE, 48000 LE, gain 0, family 0
            byteArrayOf(1, 2, 0x38, 0x01, 0x80.toByte(), 0xBB.toByte(), 0, 0, 0, 0, 0),
            head.copyOfRange(8, 19),
        )
    }

    @Test
    fun csd1AndCsd2AreLittleEndianNanoseconds() {
        assertArrayEquals(le(6_500_000L), OpusHead.csd1())
        assertArrayEquals(le(80_000_000L), OpusHead.csd2())
    }

    private fun le(v: Long) = ByteArray(8) { (v ushr (8 * it)).toByte() }
}
