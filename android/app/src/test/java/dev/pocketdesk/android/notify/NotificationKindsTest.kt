package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.NotificationKinds.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationKindsTest {
    @Test
    fun `a new kind goes first, a known one stays put`() {
        val first = NotificationKinds.seen(emptyList(), "messages", "Ann")!!
        assertEquals(listOf(Kind("messages", "Ann")), first)
        val second = NotificationKinds.seen(first, "sync", "Updating messages…")!!
        assertEquals(listOf("sync", "messages"), second.map { it.id })
        // Seen again, with the same or another title: nothing to write.
        assertNull(NotificationKinds.seen(second, "sync", "Updating messages…"))
        assertNull(NotificationKinds.seen(second, "messages", "Bob"))
        assertNull(NotificationKinds.seen(second, "messages", ""))
        assertNull(NotificationKinds.seen(second, " ", "x"))
    }

    @Test
    fun `a kind with no title takes the first it comes with`() {
        val untitled = NotificationKinds.seen(emptyList(), "sync", "")!!
        assertEquals(listOf(Kind("sync", "Syncing")), NotificationKinds.seen(untitled, "sync", "Syncing"))
    }

    @Test
    fun `an app keeps twenty kinds, the oldest going first`() {
        var kinds = emptyList<Kind>()
        repeat(NotificationKinds.MAX_KINDS + 3) { kinds = NotificationKinds.seen(kinds, "k$it", "")!! }
        assertEquals(NotificationKinds.MAX_KINDS, kinds.size)
        assertEquals("k${NotificationKinds.MAX_KINDS + 2}", kinds.first().id)
        assertEquals(false, kinds.any { it.id == "k0" })
    }

    @Test
    fun `a kind kept on the phone is never pushed out`() {
        val kept = setOf("k0")
        var kinds = emptyList<Kind>()
        repeat(NotificationKinds.MAX_KINDS * 2) { kinds = NotificationKinds.seen(kinds, "k$it", "", kept)!! }
        assertEquals(NotificationKinds.MAX_KINDS, kinds.size)
        assertTrue(kinds.any { it.id == "k0" })
    }

    @Test
    fun `a kept kind missing from the list still gets a row`() {
        val rows = NotificationKinds.listed(listOf(Kind("a", "A")), setOf("a", "lost"))
        assertEquals(listOf(Kind("a", "A"), Kind("lost", "")), rows)
    }

    @Test
    fun `an example is one short line`() {
        assertEquals("Ann", NotificationKinds.example(" Ann \nsecond line"))
        assertEquals("", NotificationKinds.example(null))
        val long = NotificationKinds.example("x".repeat(100))
        assertEquals(NotificationKinds.MAX_EXAMPLE, long.length)
        assertEquals('…', long.last())
    }
}
