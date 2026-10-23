package dev.pocketdesk.android.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class OfferHandTest {
    @Test
    fun anOfferIsTakenOnceWhileInHand() {
        val hand = OfferHand()
        assertTrue(hand.claim("a", nowMs = 0))
        assertFalse(hand.claim("a", nowMs = 1_000), "offered again while waiting to start")
        assertTrue(hand.claim("b", nowMs = 1_000))
    }

    @Test
    fun aClaimThatNeverStartedLapses() {
        val hand = OfferHand()
        assertTrue(hand.claim("a", nowMs = 0))
        assertTrue(hand.claim("a", nowMs = OfferHand.CLAIM_MS), "its tap notification has gone")
    }

    @Test
    fun aRunningFetchHoldsUntilItEnds() {
        val hand = OfferHand()
        assertTrue(hand.claim("a", nowMs = 0))
        assertTrue(hand.run("a", nowMs = 1_000))
        assertFalse(hand.run("a", nowMs = 2_000), "never fetched twice at once")
        assertFalse(hand.claim("a", nowMs = OfferHand.CLAIM_MS * 3), "running has no time limit")
        hand.release("a")
        assertTrue(hand.claim("a", nowMs = OfferHand.CLAIM_MS * 3), "ended: a later offer is taken")
    }

    @Test
    fun aFetchStartedWithoutAClaimRunsToo() {
        val hand = OfferHand()
        assertTrue(hand.run("a", nowMs = 0))
        assertFalse(hand.claim("a", nowMs = 1))
    }

    private fun assertFalse(value: Boolean, message: String) = org.junit.Assert.assertFalse(message, value)

    private fun assertTrue(value: Boolean, message: String) = org.junit.Assert.assertTrue(message, value)
}
