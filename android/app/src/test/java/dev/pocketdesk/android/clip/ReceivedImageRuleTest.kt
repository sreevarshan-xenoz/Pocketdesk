package dev.pocketdesk.android.clip

import dev.pocketdesk.android.clip.ReceivedImageRule.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ReceivedImageRuleTest {
    private fun bytes(vararg b: Int) = ByteArray(b.size) { b[it].toByte() }

    @Test
    fun `an image is known by its first bytes`() {
        assertEquals(Kind.PNG, ReceivedImageRule.kind(bytes(0x89, 'P'.code, 'N'.code, 'G'.code, 0x0D, 0x0A, 0x1A, 0x0A)))
        assertEquals(Kind.JPEG, ReceivedImageRule.kind(bytes(0xFF, 0xD8, 0xFF, 0xE0)))
        assertNull(ReceivedImageRule.kind("GIF89a..".toByteArray()))
        assertNull("a cut PNG", ReceivedImageRule.kind(bytes(0x89, 'P'.code)))
        assertNull(ReceivedImageRule.kind(ByteArray(0)))
    }

    @Test
    fun `only a file on its own, with the switch on, within the size`() {
        assertTrue(ReceivedImageRule.takes(enabled = true, onItsOwn = true, size = 1_000))
        assertFalse(ReceivedImageRule.takes(enabled = false, onItsOwn = true, size = 1_000))
        assertFalse(ReceivedImageRule.takes(enabled = true, onItsOwn = false, size = 1_000))
        assertFalse(ReceivedImageRule.takes(enabled = true, onItsOwn = true, size = 0))
        assertTrue(ReceivedImageRule.takes(enabled = true, onItsOwn = true, size = ReceivedImageRule.MAX_BYTES))
        assertFalse(ReceivedImageRule.takes(enabled = true, onItsOwn = true, size = ReceivedImageRule.MAX_BYTES + 1))
    }
}
