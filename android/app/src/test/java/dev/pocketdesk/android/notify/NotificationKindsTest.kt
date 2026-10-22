package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.NotificationKinds.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NotificationKindsTest {
    @Test
    fun `a kind seen goes to the front with its example`() {
        val first = NotificationKinds.seen(emptyList(), "messages", "Ann")!!
        assertEquals(listOf(Kind("messages", "Ann")), first)
        val second = NotificationKinds.seen(first, "sync", "Updating messages…")!!
        assertEquals(listOf("sync", "messages"), second.map { it.id })
        // The same again changes nothing; a blank example keeps the last one.
        assertNull(NotificationKinds.seen(second, "sync", "Updating messages…"))
        assertEquals(Kind("messages", "Ann"), NotificationKinds.seen(second, "messages", "")!!.first())
        assertNull(NotificationKinds.seen(second, " ", "x"))
    }

    @Test
    fun `an app keeps twenty kinds, the least recently seen going first`() {
        var kinds = emptyList<Kind>()
        repeat(NotificationKinds.MAX_KINDS + 3) { kinds = NotificationKinds.seen(kinds, "k$it", "")!! }
        assertEquals(NotificationKinds.MAX_KINDS, kinds.size)
        assertEquals("k${NotificationKinds.MAX_KINDS + 2}", kinds.first().id)
        assertEquals(false, kinds.any { it.id == "k0" })
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
