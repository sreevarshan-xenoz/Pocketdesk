package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class PrivacyReplyTest {
    @Test
    fun aLockIsDoneOnlyWhenTheComputerNamesWhatLockedIt() {
        assertEquals(
            PrivacyOutcome.Done,
            PrivacyReply.outcome(ok = true, error = null, locked = "loginctl", lockError = null, askedLock = true),
        )
        assertEquals(
            PrivacyOutcome.Refused("no screen locker found"),
            PrivacyReply.outcome(ok = true, error = null, locked = null, lockError = "no screen locker found", askedLock = true),
        )
    }

    @Test
    fun aRefusalIsNotDone() {
        val why = "Power is turned off for this phone on the computer"
        assertEquals(
            PrivacyOutcome.Refused(why),
            PrivacyReply.outcome(ok = false, error = why, locked = null, lockError = null, askedLock = true),
        )
        assertEquals(
            "a grab another phone holds",
            PrivacyOutcome.Refused("another phone holds privacy mode"),
            PrivacyReply.outcome(ok = false, error = "another phone holds privacy mode", locked = null, lockError = null, askedLock = false),
        )
    }

    @Test
    fun aGrabWithoutALockNeedsOnlyOk() {
        assertEquals(
            PrivacyOutcome.Done,
            PrivacyReply.outcome(ok = true, error = null, locked = null, lockError = null, askedLock = false),
        )
    }
}
