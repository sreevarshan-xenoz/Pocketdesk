package dev.pocketdesk.android.stream

import org.junit.Assert.assertEquals
import org.junit.Test

class TalkTimerTest {
    @Test
    fun theTimeLeftReadsAsMinutesAndSecondsThenOver() {
        assertEquals("20:00", TalkTimer.text(1_200))
        assertEquals("0:05", TalkTimer.text(5))
        assertEquals("0:00", TalkTimer.text(0))
        assertEquals("+1:02", TalkTimer.text(-62))
        assertEquals("61:00", TalkTimer.text(3_660))
    }

    @Test
    fun aLongTalkWarnsAtFiveMinutesAtOneAndAtTheEnd() {
        assertEquals(listOf(300L to 1, 60L to 2, 0L to 3), TalkTimer.marks(1_200))
        // A short talk has no five-minute warning: it would come at once.
        assertEquals(listOf(60L to 2, 0L to 3), TalkTimer.marks(300))
        assertEquals(listOf(0L to 3), TalkTimer.marks(60))
    }

    @Test
    fun aMarkBuzzesOnceAsItIsCrossed() {
        val total = 1_200L
        assertEquals(0, TalkTimer.pulses(total, before = 302, now = 301))
        assertEquals(1, TalkTimer.pulses(total, before = 301, now = 300))
        assertEquals(0, TalkTimer.pulses(total, before = 300, now = 299))
        assertEquals(2, TalkTimer.pulses(total, before = 61, now = 59))
        assertEquals(3, TalkTimer.pulses(total, before = 1, now = -1))
        // A long sleep crossing two marks buzzes the later one.
        assertEquals(2, TalkTimer.pulses(total, before = 400, now = 50))
    }

    @Test
    fun theChoicesAreTheUsualTalkLengths() {
        assertEquals(listOf(5, 10, 15, 20, 30, 45, 60), TalkTimer.MINUTES)
    }
}
