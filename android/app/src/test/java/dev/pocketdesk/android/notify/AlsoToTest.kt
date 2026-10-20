package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AlsoToTest {
    private val note = "kdeconnect.notification"

    @Test
    fun onlyPickedPairedOtherComputersGetIt() {
        val saved = listOf("own", "b", "c", "unpaired", "d", "e", "b")
        val paired = { a: String -> a != "unpaired" }
        val picked = setOf("own", "b", "c", "unpaired", "d", "e")
        assertEquals(listOf("b", "c", "d"), AlsoTo.targets(note, saved, paired, picked, own = "own"))
        assertEquals(listOf("b"), AlsoTo.targets("kdeconnect.sms.messages", listOf("b"), paired, setOf("b"), "own"))
        assertTrue("not picked", AlsoTo.targets(note, saved, paired, emptySet(), "own").isEmpty())
        assertTrue("the battery is the own computer's", AlsoTo.targets("kdeconnect.battery", saved, paired, picked, "own").isEmpty())
    }

    @Test
    fun aCopyLeavesOutWhatOffersAnAnswer() {
        // org.json is Android's, not in JVM tests: the names it drops are held here.
        assertEquals(listOf("actions", "requestReplyId", "messages"), AlsoTo.ANSWERS)
    }
}
