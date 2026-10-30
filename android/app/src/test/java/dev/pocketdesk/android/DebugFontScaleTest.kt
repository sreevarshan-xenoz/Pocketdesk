package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DebugFontScaleTest {
    @Test
    fun `reads the scales the checks use`() {
        assertEquals(0.85f, DebugFontScale.parse("0.85\n")!!, 0f)
        assertEquals(1.3f, DebugFontScale.parse(" 1.3 ")!!, 0f)
    }

    @Test
    fun `no file or nonsense keeps the system's`() {
        assertNull(DebugFontScale.parse(null))
        assertNull(DebugFontScale.parse(""))
        assertNull(DebugFontScale.parse("big"))
        assertNull(DebugFontScale.parse("0"))
        assertNull(DebugFontScale.parse("9"))
    }
}
