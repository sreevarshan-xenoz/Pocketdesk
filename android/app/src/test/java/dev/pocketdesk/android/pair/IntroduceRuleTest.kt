package dev.pocketdesk.android.pair

import dev.pocketdesk.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class IntroduceRuleTest {
    @Test
    fun aComputerThisPhoneHasIsNeverOfferedAndEachOnce() {
        val offers = listOf(
            IntroduceRule.Offer("b", "Arch"),
            IntroduceRule.Offer("c", "Office"),
            IntroduceRule.Offer("b", "Arch again"),
            IntroduceRule.Offer(" ", "No id"),
        )
        assertEquals(
            listOf(IntroduceRule.Offer("b", "Arch")),
            IntroduceRule.fresh(offers, setOf("c")),
        )
        assertTrue(IntroduceRule.fresh(offers, setOf("b", "c")).isEmpty())
    }

    @Test
    fun aComputerWithoutANameShowsTheStartOfItsId() {
        assertEquals("Arch", IntroduceRule.shownName(IntroduceRule.Offer("b", " Arch ")))
        assertEquals("01234567", IntroduceRule.shownName(IntroduceRule.Offer("0123456789", "")))
    }

    @Test
    fun eachRefusalHasItsWordsAndAnUnknownOneFails() {
        assertEquals(R.string.introduce_refused_off, IntroduceRule.refusal("off"))
        assertEquals(R.string.introduce_refused_taken, IntroduceRule.refusal("taken"))
        assertEquals(R.string.introduce_refused_not_owner, IntroduceRule.refusal("not_owner"))
        assertEquals(R.string.introduce_refused_no_answer, IntroduceRule.refusal("no_answer"))
        assertEquals(R.string.introduce_refused_failed, IntroduceRule.refusal("something new"))
        assertEquals(R.string.introduce_refused_failed, IntroduceRule.refusal(null))
    }

    @Test
    fun onlyAPairingLinkForTheComputerAskedIsTaken() {
        assertTrue(IntroduceRule.isPairLink("pocketdesk://pair?v=1&id=b"))
        assertFalse(IntroduceRule.isPairLink("https://example.com/"))
        assertFalse(IntroduceRule.isPairLink(null))
        val link = PairLink("Arch", "b", listOf("192.0.2.20"), 1716, 4716, "042917", "ab".repeat(32), null)
        assertTrue(IntroduceRule.linkFits(link, "b"))
        assertFalse(IntroduceRule.linkFits(link, "c"))
    }

    @Test
    fun onlyTheAnswerToThisAskIsTaken() {
        val asked = IntroduceRule.newRequest()
        assertTrue(IntroduceRule.isAnswerTo(asked, asked))
        assertFalse(IntroduceRule.isAnswerTo(asked, "other"))
        assertFalse(IntroduceRule.isAnswerTo("", ""))
        assertFalse(asked == IntroduceRule.newRequest())
    }

    @Test
    fun theLastOutcomeReadsBackAsKept() {
        for (outcome in listOf(
            IntroduceRule.Outcome(IntroduceRule.Kind.PAIRED, "Arch"),
            IntroduceRule.Outcome(IntroduceRule.Kind.REFUSED, "Office\nPC", "off"),
            IntroduceRule.Outcome(IntroduceRule.Kind.UNREACHABLE, ""),
        )) {
            val read = IntroduceRule.decode(IntroduceRule.encode(outcome))
            assertEquals(outcome.copy(name = outcome.name.replace('\n', ' ')), read)
        }
        assertNull(IntroduceRule.decode(null))
        assertNull(IntroduceRule.decode("NOPE\n\nx"))
        assertNull(IntroduceRule.decode("PAIRED"))
    }
}
