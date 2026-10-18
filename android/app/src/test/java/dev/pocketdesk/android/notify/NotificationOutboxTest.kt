package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationOutboxTest {
    /** A computer that takes what it is sent while [up]. */
    private class Computer {
        var up = true
        val shown = mutableListOf<String>()
        val waited = mutableListOf<Long>()
        fun send(body: String, waitedMs: Long): Boolean = up.also {
            if (it) {
                shown += body
                waited += waitedMs
            }
        }
    }

    private val any = { _: String, _: String -> true }

    @Test
    fun `a notification goes at once when the computer answers`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer()
        outbox.post("k", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        assertEquals(listOf("A"), computer.shown)
    }

    @Test
    fun `one sent from the wait says how long it waited, one sent at once says none`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("a", "app", "A", nowMs = 1_000, sendable = any, send = computer::send)
        computer.up = true
        outbox.flush(nowMs = 61_000, sendable = any, send = computer::send)
        outbox.post("b", "app", "B", nowMs = 62_000, sendable = any, send = computer::send)
        assertEquals(listOf("A", "B"), computer.shown)
        assertEquals(listOf(60_000L, 0L), computer.waited)
    }

    @Test
    fun `with no wait a notification is sent at once or given up on, never queued`() {
        val outbox = NotificationOutbox<String>(PendingNotifications(maxAgeMs = 0))
        val computer = Computer().apply { up = false }
        assertEquals(listOf("a"), outbox.post("a", "app", "A", nowMs = 0, sendable = any, send = computer::send))
        assertTrue(outbox.isEmpty())
        computer.up = true
        assertEquals(emptyList<String>(), outbox.post("b", "app", "B", nowMs = 1, sendable = any, send = computer::send))
        assertEquals(listOf("B"), computer.shown)
    }

    @Test
    fun `turning waiting off gives up on what waited`() {
        val outbox = NotificationOutbox<String>(PendingNotifications(maxAgeMs = 60_000))
        val computer = Computer().apply { up = false }
        outbox.post("a", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        outbox.maxAgeMs = 0
        computer.up = true
        assertEquals(listOf("a"), outbox.post("b", "app", "B", nowMs = 1_000, sendable = any, send = computer::send))
        assertEquals(listOf("B"), computer.shown)
    }

    @Test
    fun `an update cannot be overtaken by the older version that waited`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("k", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        computer.up = true
        outbox.post("k", "app", "B", nowMs = 1_000, sendable = any, send = computer::send)
        outbox.flush(nowMs = 2_000, sendable = any, send = computer::send)
        assertEquals("the computer ends on the newest", "B", computer.shown.last())
        assertFalse("A" in computer.shown)
    }

    @Test
    fun `newer notifications wait behind older ones rather than overtake them`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("a", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        computer.up = true
        outbox.post("b", "app", "B", nowMs = 1_000, sendable = any, send = computer::send)
        assertEquals(listOf("A", "B"), computer.shown)
    }

    @Test
    fun `a close reaches the computer for one it was shown, even if an update to it expired`() {
        val outbox = NotificationOutbox<String>(PendingNotifications(maxAgeMs = 10_000))
        val computer = Computer()
        outbox.post("k", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        computer.up = false
        outbox.post("k", "app", "B", nowMs = 1_000, sendable = any, send = computer::send)
        outbox.flush(nowMs = 20_000, sendable = any, send = computer::send)
        assertTrue("A is still on the computer, so it must be closed there", outbox.closed("k"))
    }

    @Test
    fun `one closed before it ever went out is dropped, with nothing to close`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("k", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        assertFalse(outbox.closed("k"))
        computer.up = true
        outbox.flush(nowMs = 1_000, sendable = any, send = computer::send)
        assertTrue(computer.shown.isEmpty())
    }

    @Test
    fun `one that may no longer be sent is dropped from the queue`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("k", "chat", "A", nowMs = 0, sendable = any, send = computer::send)
        computer.up = true
        val forgotten = outbox.flush(nowMs = 1_000, sendable = { _, pkg -> pkg != "chat" }, send = computer::send)
        assertEquals(listOf("k"), forgotten)
        assertTrue(computer.shown.isEmpty())
    }

    @Test
    fun `sending what waited stops at the first failure and keeps the rest`() {
        val outbox = NotificationOutbox<String>()
        val computer = Computer().apply { up = false }
        outbox.post("a", "app", "A", nowMs = 0, sendable = any, send = computer::send)
        outbox.post("b", "app", "B", nowMs = 0, sendable = any, send = computer::send)
        outbox.flush(nowMs = 1_000, sendable = any, send = computer::send)
        computer.up = true
        outbox.flush(nowMs = 2_000, sendable = any, send = computer::send)
        assertEquals(listOf("A", "B"), computer.shown)
    }
}
