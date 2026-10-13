package dev.pocketdesk.android.service

import dev.pocketdesk.android.service.TransferSummary.Outcome
import org.junit.Assert.assertEquals
import org.junit.Test

class TransferSummaryTest {
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
}
