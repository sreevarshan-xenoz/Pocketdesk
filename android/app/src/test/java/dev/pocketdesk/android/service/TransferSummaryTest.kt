package dev.pocketdesk.android.service

import dev.pocketdesk.android.service.TransferSummary.Outcome
import org.junit.Assert.assertEquals
import org.junit.Test

class TransferSummaryTest {
    @Test
    fun aFolderThatLeftNamesOutSaysSoEvenWhenTheRestWent() {
        val left = "Not all of Trip was sent. Left out: 2"
        assertEquals(left, TransferSummary.detail(failed = false, lastError = null, notice = left))
        assertEquals(null, TransferSummary.detail(failed = false, lastError = "old", notice = null))
        assertEquals("reset", TransferSummary.detail(failed = true, lastError = "reset", notice = left))
        assertEquals(left, TransferSummary.detail(failed = true, lastError = null, notice = left))
    }

    @Test
    fun aBatchThatOnlySentSaysSo() {
        assertEquals(Outcome.Sent(3), TransferSummary.of(sent = 3, sends = 3, saved = 0, receives = 0))
    }

    @Test
    fun aBatchThatOnlySavedSaysSo() {
        assertEquals(Outcome.Saved(1), TransferSummary.of(sent = 0, sends = 0, saved = 1, receives = 1))
    }

    @Test
    fun aMixedBatchCountsBothWays() {
        assertEquals(Outcome.Mixed(2, 1), TransferSummary.of(sent = 2, sends = 2, saved = 1, receives = 1))
    }

    @Test
    fun anyFailureIsWhatTheSummaryLeadsWith() {
        assertEquals(
            "a failed send alone keeps the send wording",
            Outcome.NotSent(1, 3),
            TransferSummary.of(sent = 2, sends = 3, saved = 0, receives = 0),
        )
        assertEquals(
            Outcome.Failed(1, 4),
            TransferSummary.of(sent = 3, sends = 3, saved = 0, receives = 1),
        )
        assertEquals(
            Outcome.Failed(1, 1),
            TransferSummary.of(sent = 0, sends = 0, saved = 0, receives = 1),
        )
    }

    @Test
    fun aSendCutShortCountsWhatItDidNotSendAsFailed() {
        assertEquals("all went", 0, TransferSummary.failedOf(planned = 3, counted = 3, sent = 3, threw = false))
        assertEquals("cut off after 3 of 20", 17, TransferSummary.failedOf(planned = 20, counted = 3, sent = 3, threw = false))
        assertEquals("threw before it knew", 1, TransferSummary.failedOf(planned = 0, counted = 0, sent = 0, threw = true))
        assertEquals("an empty folder", 1, TransferSummary.failedOf(planned = 0, counted = 1, sent = 0, threw = false))
        assertEquals("one retried and failed", 1, TransferSummary.failedOf(planned = 2, counted = 2, sent = 1, threw = false))
    }
}
