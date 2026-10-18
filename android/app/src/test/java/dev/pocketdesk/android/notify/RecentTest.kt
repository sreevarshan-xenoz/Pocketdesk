package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Test

class RecentTest {
    /** N7: the background timers' answer is worked out again only when it is old. */
    @Test
    fun `a value is kept for its time, then worked out again`() {
        val recent = Recent<String?>(ttlMs = 5_000L)
        var computed = 0
        fun get(now: Long) = recent.get(now) { computed++; "host$computed" }
        assertEquals("host1", get(1_000))
        assertEquals("host1", get(5_999))
        assertEquals(1, computed)
        assertEquals("host2", get(6_000))
        assertEquals(2, computed)
        // A null answer (no computer yet) is kept too.
        val none = Recent<String?>(ttlMs = 5_000L)
        var asked = 0
        none.get(0) { asked++; null }
        none.get(100) { asked++; null }
        assertEquals(1, asked)
        // Forgotten on demand (a computer added or removed).
        recent.forget()
        assertEquals("host3", get(6_100))
    }
}
