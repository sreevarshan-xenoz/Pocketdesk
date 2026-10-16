package dev.pocketdesk.android.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class PhoneEventsTest {
    @Test
    fun anEventNamesItsComputerAndKind() {
        val event = PhoneEvents.build(PhoneEvents.Kind.SESSION_STARTED, "192.0.2.20", mapOf("mode" to "screen"))!!
        assertEquals("dev.pocketdesk.android.event.SESSION_STARTED", event.kind.action)
        assertEquals("session_started", event.extras[PhoneEvents.EXTRA_EVENT])
        assertEquals("192.0.2.20", event.extras[PhoneEvents.EXTRA_ADDRESS])
        assertEquals("screen", event.extras["dev.pocketdesk.android.extra.MODE"])
    }

    @Test
    fun onlyTheFieldsAKindAllowsGoOut() {
        val file = PhoneEvents.build(
            PhoneEvents.Kind.FILE_RECEIVED,
            "192.0.2.20",
            mapOf("mime" to "image/png", "size" to 1234L, "name" to "secret-plans.pdf", "uri" to "content://x"),
        )!!
        assertEquals("image/png", file.extras["dev.pocketdesk.android.extra.MIME"])
        assertEquals(1234L, file.extras["dev.pocketdesk.android.extra.SIZE"])
        assertFalse("no file name", file.extras.values.any { it == "secret-plans.pdf" })
        assertFalse("no uri", file.extras.keys.any { it.endsWith("URI") || it.endsWith("NAME") })
        val link = PhoneEvents.build(PhoneEvents.Kind.LINK_RECEIVED, "192.0.2.20", mapOf("url" to "https://example.org/private"))!!
        assertEquals("no link address", 2, link.extras.size)
        val task = PhoneEvents.build(
            PhoneEvents.Kind.TASK_FINISHED,
            "192.0.2.20",
            mapOf("task" to "Back up", "code" to 0, "timed_out" to false, "output" to "password=hunter2"),
        )!!
        assertEquals("Back up", task.extras["dev.pocketdesk.android.extra.TASK"])
        assertEquals(0, task.extras["dev.pocketdesk.android.extra.CODE"])
        assertFalse("no output", task.extras.values.any { it.toString().contains("hunter2") })
    }

    @Test
    fun aBlankAddressOrAnOddValueIsLeftOut() {
        assertNull(PhoneEvents.build(PhoneEvents.Kind.SESSION_ENDED, " "))
        val event = PhoneEvents.build(PhoneEvents.Kind.SESSION_ENDED, "pc.local", mapOf("reason" to listOf("x")))!!
        assertEquals(2, event.extras.size)
    }

    @Test
    fun everyKindHasItsFields() {
        for (kind in PhoneEvents.Kind.entries) {
            PhoneEvents.FIELDS.getValue(kind)
        }
    }
}
