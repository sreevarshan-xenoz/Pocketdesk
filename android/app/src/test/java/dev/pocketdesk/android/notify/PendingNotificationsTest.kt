package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PendingNotificationsTest {
    @Test
    fun `a notification that could not be sent waits, oldest first`() {
        val pending = PendingNotifications<String>()
        pending.add("a", "first", nowMs = 0)
        pending.add("b", "second", nowMs = 1_000)
        assertEquals(listOf("a" to "first", "b" to "second"), pending.due(nowMs = 2_000).send)
    }

    @Test
    fun `an update replaces the waiting one and keeps its place`() {
        val pending = PendingNotifications<String>()
        pending.add("a", "first", nowMs = 0)
        pending.add("b", "second", nowMs = 0)
        pending.add("a", "first, edited", nowMs = 500)
        assertEquals(listOf("a" to "first, edited", "b" to "second"), pending.waiting())
    }

    @Test
    fun `one closed on the phone before it went out is dropped, and says so`() {
        val pending = PendingNotifications<String>()
        pending.add("a", "first", nowMs = 0)
        assertTrue(pending.remove("a"))
        assertFalse(pending.remove("a"))
        assertTrue(pending.waiting().isEmpty())
    }

    @Test
    fun `each one due says how long it waited, an update from when it was posted`() {
        val pending = PendingNotifications<String>(maxAgeMs = 8_500)
        pending.add("a", "first", nowMs = 1_000)
        pending.add("b", "second", nowMs = 4_000)
        pending.add("a", "first, edited", nowMs = 5_000)
        val due = pending.due(nowMs = 9_000)
        assertEquals(mapOf("a" to 4_000L, "b" to 5_000L), due.waited)
        // Its place and its age are still the first version's.
        assertEquals(listOf("a", "b"), due.send.map { it.first })
        assertEquals(listOf("a"), pending.due(nowMs = 9_500).expired)
    }

    @Test
    fun `how long they may wait follows the setting`() {
        val pending = PendingNotifications<String>(maxAgeMs = 60_000)
        pending.add("a", "x", nowMs = 0)
        pending.maxAgeMs = 10_000
        assertEquals(listOf("a"), pending.due(nowMs = 20_000).expired)
        pending.maxAgeMs = 0
        pending.add("b", "y", nowMs = 30_000)
        assertEquals("not waiting at all", listOf("b"), pending.due(nowMs = 30_000).expired)
    }

    @Test
    fun `old ones expire instead of arriving late`() {
        val pending = PendingNotifications<String>(maxAgeMs = 10_000)
        pending.add("old", "x", nowMs = 0)
        pending.add("new", "y", nowMs = 8_000)
        val due = pending.due(nowMs = 12_000)
        assertEquals(listOf("old"), due.expired)
        assertEquals(listOf("new" to "y"), due.send)
        assertEquals(listOf("new" to "y"), pending.waiting())
    }

    @Test
    fun `at most so many wait, and the oldest gives way`() {
        val pending = PendingNotifications<String>(capacity = 2)
        assertEquals(null, pending.add("a", "1", nowMs = 0))
        assertEquals(null, pending.add("b", "2", nowMs = 0))
        assertEquals("a", pending.add("c", "3", nowMs = 0))
        assertEquals(listOf("b" to "2", "c" to "3"), pending.waiting())
    }

    @Test
    fun `a sent one leaves the queue`() {
        val pending = PendingNotifications<String>()
        pending.add("a", "1", nowMs = 0)
        pending.sent("a")
        assertTrue(pending.waiting().isEmpty())
    }
}
