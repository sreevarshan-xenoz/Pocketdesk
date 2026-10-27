package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Test

class NoteRuleTest {
    @Test
    fun theComputersAnswersAreKnown() {
        assertEquals(NoteRule.Outcome.Saved, NoteRule.outcome(null))
        assertEquals(NoteRule.Outcome.NoFile, NoteRule.outcome("no_file"))
        assertEquals(NoteRule.Outcome.Empty, NoteRule.outcome("empty"))
        assertEquals(NoteRule.Outcome.NotWritten, NoteRule.outcome("not_written"))
        assertEquals(NoteRule.Outcome.NotAllowed, NoteRule.outcome("not_allowed"))
    }

    @Test
    fun anythingElseIsAFailureSaidAsItCame() {
        assertEquals(NoteRule.Outcome.Failed, NoteRule.outcome("Connection refused"))
        assertEquals(NoteRule.Outcome.Failed, NoteRule.outcome(""))
    }
}
