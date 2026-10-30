package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class SettingsJumpsTest {
    @Test
    fun groupsComeTopToBottom() {
        val jumps = SettingsJumps()
        jumps.tops["Wake"] = 900
        jumps.tops["This phone"] = 0
        jumps.tops["Session"] = 400
        assertEquals(listOf("This phone" to 0, "Session" to 400, "Wake" to 900), jumps.ordered())
    }

    @Test
    fun noneBeforeAnyHeadingIsDrawn() {
        assertEquals(emptyList<Pair<String, Int>>(), SettingsJumps().ordered())
    }
}
