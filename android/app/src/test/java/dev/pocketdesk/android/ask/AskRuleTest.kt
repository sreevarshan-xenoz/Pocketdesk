package dev.pocketdesk.android.ask

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AskRuleTest {
    @Test
    fun onlyTheOwnComputersWaitingQuestionIsOffered() {
        assertEquals(AskRule.Decision.Offer, AskRule.decide(on = true, ownComputer = true, expiresUnix = 120, nowUnix = 100))
        assertEquals(AskRule.Decision.Off, AskRule.decide(on = false, ownComputer = true, expiresUnix = 120, nowUnix = 100))
        assertEquals(
            AskRule.Decision.NotOwnComputer,
            AskRule.decide(on = true, ownComputer = false, expiresUnix = 120, nowUnix = 100),
        )
        assertEquals(AskRule.Decision.Expired, AskRule.decide(on = true, ownComputer = true, expiresUnix = 120, nowUnix = 120))
    }

    @Test
    fun yesTakesALockAndNoDoesNot() {
        assertTrue(AskRule.mayAnswerYes(deviceSecure = true))
        assertFalse(AskRule.mayAnswerYes(deviceSecure = false))
    }

    @Test
    fun theAnswerNamesItsQuestionAndNothingElse() {
        assertEquals("""{"id":"q-1","answer":"yes"}""", AskRule.body("q-1", yes = true))
        assertEquals("""{"id":"q-1","answer":"no"}""", AskRule.body("q-1", yes = false))
        assertNull(AskRule.body("""q","answer":"yes""", yes = false))
        assertNull(AskRule.body("", yes = true))
    }
}
