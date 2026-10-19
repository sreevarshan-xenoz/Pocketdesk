package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.CallRule.Act
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CallRuleTest {
    @Test
    fun aCallIsShownByNameAndNumberWithItsButtons() {
        val note = CallRule.note(7, "+15550100", "Ann", mayDecline = true)
        assertEquals("pocketdesk:call:7", note.id)
        assertEquals("Call from Ann (+15550100)", note.title)
        assertEquals(listOf("Silence", "Decline"), note.actions)
        assertEquals("Call from +15550100", CallRule.note(1, "+15550100", null, mayDecline = false).title)
        assertEquals(listOf("Silence"), CallRule.note(1, "+15550100", null, mayDecline = false).actions)
        assertEquals("Call from an unknown number", CallRule.note(1, null, null, mayDecline = false).title)
        assertTrue(CallRule.isCall(note.id))
        assertFalse(CallRule.isCall("0|com.example|1|null|10001"))
    }

    @Test
    fun anAnswerActsOnlyOnTheCallRingingNowFromTheOwnComputer() {
        val ringing = "pocketdesk:call:7"
        fun decide(key: String = ringing, action: String? = "Silence", now: String? = ringing, own: Boolean = true, on: Boolean = true, decline: Boolean = true) =
            CallRule.decide(key, action, now, own, on, decline)
        assertEquals(Act.Silence, decide())
        assertEquals(Act.Decline, decide(action = "Decline"))
        assertEquals(Act.NotOffered, decide(action = "Decline", decline = false))
        assertEquals(Act.NotOffered, decide(action = "Answer"))
        assertEquals(Act.Gone, decide(now = null))
        assertEquals(Act.Gone, decide(key = "pocketdesk:call:6"))
        assertEquals(Act.Off, decide(on = false))
        assertEquals(Act.NotOwnComputer, decide(own = false))
        assertEquals(null, CallRule.refusal(Act.NotOwnComputer))
        assertEquals("the call has stopped ringing", CallRule.refusal(Act.Gone))
    }
}
