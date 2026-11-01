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
    fun aCardItsComputerNoLongerPairsWithIsNotCountedAsPaired() {
        val cards = mapOf("192.0.2.1" to "a", "192.0.2.2" to "b", "192.0.2.3" to null)
        val unpaired = setOf("192.0.2.2")
        val paired = IntroduceRule.pairedIds(cards) { it in unpaired }
        assertEquals(setOf("a"), paired)
        assertEquals(
            listOf(IntroduceRule.Offer("b", "Arch")),
            IntroduceRule.fresh(listOf(IntroduceRule.Offer("a", "Office"), IntroduceRule.Offer("b", "Arch")), paired),
        )
        assertEquals(setOf("a", "b"), IntroduceRule.pairedIds(cards) { false })
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
    fun onlyTheLinkAnsweringThisAskForThatComputerIsTaken() {
        val link = "pocketdesk://pair?v=1&name=Arch&id=b&a=192.0.2.20&cp=1716&sp=4716&pin=042917&fp=" + "ab".repeat(32)
        // This ask's answer, for the computer asked: taken.
        val taken = IntroduceRule.takeLink("ask-2", "ask-2", "b", link)
        assertEquals("b", taken?.deviceId)
        assertEquals("042917", taken?.pin)
        // A late answer to an earlier ask is not this one's.
        assertNull(IntroduceRule.takeLink("ask-2", "ask-1", "b", link))
        assertNull(IntroduceRule.takeLink("", "", "b", link))
        // A link to another computer than the one asked for.
        assertNull(IntroduceRule.takeLink("ask-2", "ask-2", "c", link))
        // Not a pairing link, or one that does not read.
        assertNull(IntroduceRule.takeLink("ask-2", "ask-2", "b", "https://example.com/"))
        assertNull(IntroduceRule.takeLink("ask-2", "ask-2", "b", "pocketdesk://pair?v=1&id=b"))
        assertNull(IntroduceRule.takeLink("ask-2", "ask-2", "b", null))
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

    @Test
    fun notNowKeepsAComputersOwnOfferQuietForTheCoolDown() {
        val gate = IntroduceRule.Gate(quietMs = 1_000)
        assertTrue(gate.mayOfferByItself("192.0.2.1", now = 0))
        gate.notNow("192.0.2.1", now = 100)
        assertFalse("just passed", gate.mayOfferByItself("192.0.2.1", now = 100))
        assertFalse(gate.mayOfferByItself("192.0.2.1", now = 1_099))
        assertTrue("another computer is not quieted", gate.mayOfferByItself("192.0.2.2", now = 200))
        assertTrue("after the cool-down", gate.mayOfferByItself("192.0.2.1", now = 1_100))
        // A clock set back does not quiet it for ever.
        assertTrue(gate.mayOfferByItself("192.0.2.1", now = 50))
    }

    @Test
    fun anOfferThatMustWaitIsKeptNotLost() {
        assertEquals(IntroduceRule.Pending.Now, IntroduceRule.pendingStep(offerOn = true, sheetOpen = false, quietLeftMs = 0))
        assertEquals(IntroduceRule.Pending.Hold, IntroduceRule.pendingStep(offerOn = true, sheetOpen = true, quietLeftMs = 0))
        assertEquals(IntroduceRule.Pending.Hold, IntroduceRule.pendingStep(offerOn = true, sheetOpen = true, quietLeftMs = 5))
        assertEquals(IntroduceRule.Pending.After(5), IntroduceRule.pendingStep(offerOn = true, sheetOpen = false, quietLeftMs = 5))
        assertEquals(IntroduceRule.Pending.Drop, IntroduceRule.pendingStep(offerOn = false, sheetOpen = true, quietLeftMs = 5))
    }

    @Test
    fun quietLeftCountsDownFromNotNow() {
        val gate = IntroduceRule.Gate(quietMs = 1_000)
        assertEquals(0L, gate.quietLeftMs("192.0.2.1", now = 0))
        gate.notNow("192.0.2.1", now = 100)
        assertEquals(1_000L, gate.quietLeftMs("192.0.2.1", now = 100))
        assertEquals(400L, gate.quietLeftMs("192.0.2.1", now = 700))
        assertEquals(0L, gate.quietLeftMs("192.0.2.1", now = 1_100))
        assertEquals("a clock set back", 0L, gate.quietLeftMs("192.0.2.1", now = 50))
    }

    @Test
    fun twoSheetsNeverStack() {
        val gate = IntroduceRule.Gate()
        assertTrue(gate.open("192.0.2.1"))
        assertFalse("a second computer's sheet waits", gate.open("192.0.2.2"))
        assertFalse("no offer by itself over an open sheet", gate.mayOfferByItself("192.0.2.2", now = 0))
        assertTrue("the same sheet composed again keeps its place", gate.open("192.0.2.1"))
        gate.closed("192.0.2.2")
        assertFalse("only the open sheet's close frees it", gate.open("192.0.2.2"))
        gate.closed("192.0.2.1")
        assertTrue(gate.open("192.0.2.2"))
    }
}
