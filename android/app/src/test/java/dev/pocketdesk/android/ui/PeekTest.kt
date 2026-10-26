package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class PeekTest {
    @Test
    fun theComputersReasonsAreKnown() {
        assertEquals(Peek.Refusal.Off, Peek.refusal("off"))
        assertEquals(Peek.Refusal.Paused, Peek.refusal("paused"))
        assertEquals(Peek.Refusal.Busy, Peek.refusal("busy"))
        assertEquals(Peek.Refusal.TooSoon, Peek.refusal("too_soon"))
        assertEquals(Peek.Refusal.NoScreen, Peek.refusal("no_screen"))
        assertEquals(Peek.Refusal.NotAllowed, Peek.refusal("not_allowed"))
    }

    @Test
    fun anythingElseIsShownAsItCame() {
        assertEquals(Peek.Refusal.Other, Peek.refusal(null))
        assertEquals(Peek.Refusal.Other, Peek.refusal("Connection refused"))
        assertEquals(
            Peek.Refusal.Other,
            Peek.refusal("The computer refused the download: no free transfer slot"),
        )
    }
}
