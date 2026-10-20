package dev.pocketdesk.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FileDatesTest {
    /** K1: a share sends its date only with the setting on and a date known. */
    @Test
    fun `a share sends its date when it has one`() {
        assertEquals(1_780_000_000_000L, FileDates.toSend(true, 1_780_000_000_000L))
        assertEquals("off", 0L, FileDates.toSend(false, 1_780_000_000_000L))
        assertEquals("unknown", 0L, FileDates.toSend(true, null))
        assertEquals("nonsense", 0L, FileDates.toSend(true, -5L))
    }

    /** K1 review: a file downloaded from the browser takes its listing's date. */
    @Test
    fun `a listing's seconds become milliseconds`() {
        assertEquals(1_780_000_000_000L, FileDates.fromListing(1_780_000_000L))
        assertEquals(0L, FileDates.fromListing(0L))
        assertEquals(0L, FileDates.fromListing(-1L))
    }

    /** K1: a received file keeps the offer's date, never one after now. */
    @Test
    fun `a received file keeps its date up to now`() {
        val now = 1_800_000_000_000L
        assertEquals(1_780_000_000_000L, FileDates.toKeep(true, 1_780_000_000_000L, now))
        assertEquals("a date after now is now", now, FileDates.toKeep(true, now + 1, now))
        assertNull("off", FileDates.toKeep(false, 1_780_000_000_000L, now))
        assertNull("none", FileDates.toKeep(true, 0L, now))
    }
}
