package dev.pocketdesk.android.cast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WebcamAskTest {
    /** F6 part 2: what the computer asks for is held to what may be asked. */
    @Test
    fun anAskIsHeldToWhatMayBeAsked() {
        assertEquals(WebcamAsk.Ask(true, 1080, 15), WebcamAsk.Ask.of("FRONT", 9999, 1))
        assertEquals(WebcamAsk.Ask(false, 720, 30), WebcamAsk.Ask.of("side", 0, 120))
        assertEquals(WebcamAsk.Ask(false, 1080, 24), WebcamAsk.Ask.of("back", 1080, 24))
        assertEquals("a tie in height goes low", 720, WebcamAsk.Ask.of("back", 900, 30).height)
    }

    /** Only with the switch on, and only from the phone's own computer. */
    @Test
    fun anAskIsOfferedOnlyWithTheSwitchFromItsOwnComputer() {
        assertTrue(WebcamAsk.shouldOffer(enabled = true, fromOwnComputer = true))
        assertFalse(WebcamAsk.shouldOffer(enabled = false, fromOwnComputer = true))
        assertFalse(WebcamAsk.shouldOffer(enabled = true, fromOwnComputer = false))
        assertFalse(WebcamAsk.shouldOffer(enabled = false, fromOwnComputer = false))
    }
}
