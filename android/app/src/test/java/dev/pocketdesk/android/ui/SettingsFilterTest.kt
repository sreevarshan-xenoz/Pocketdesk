package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** O8 review: a group's heading and "No setting matches" follow the rows as drawn. */
class SettingsFilterTest {
    @Test
    fun `a group counts its rows shown, and nothing found only while searching`() {
        val filter = SettingsFilter()
        val (a, b, c) = Triple(Any(), Any(), Any())
        filter.rows[a] = "Session" to true
        filter.rows[b] = "Session" to false
        filter.rows[c] = "Wake" to false
        assertEquals(1, filter.found("Session"))
        assertEquals(0, filter.found("Wake"))
        assertFalse("not searching", filter.nothingFound())
        filter.query = "zzz"
        assertFalse("a row still shows", filter.nothingFound())
        filter.rows[a] = "Session" to false
        assertTrue(filter.nothingFound())
        filter.rows.remove(a)
        assertEquals(0, filter.found("Session"))
    }
}
