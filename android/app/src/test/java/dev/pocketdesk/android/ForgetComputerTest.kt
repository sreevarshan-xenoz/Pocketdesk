package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ForgetComputerTest {
    private val home = java.io.File("src/main/java/dev/pocketdesk/android/ui/HomeScreen.kt").readText()

    /**
     * BC1: the doctor's Pair again says it removes the computer, but did
     * less than Remove (its link, its deck list and its "also send to"
     * pick stayed). Both now forget it the one way.
     */
    @Test
    fun `remove and the doctor's pair again forget a computer the same way`() {
        assertEquals("Remove and Pair again", 2, Regex("""ForgetComputer\.forget\(""").findAll(home).count())
        assertFalse("Home removes a computer only through ForgetComputer", home.contains("HostStore.remove("))
        // Review: Remove's own call is kept, and Pair again scans only once
        // the old key is forgotten (its new code's key is trusted then).
        val removing = home.substringAfter("removing?.let { host ->")
        assertTrue("Remove forgets", removing.substringBefore("dismissButton").contains("ForgetComputer.forget("))
        val rescan = home.substringAfter("rescanning?.let { host ->").substringBefore("dismissButton")
        val joined = rescan.indexOf("forgetting.join()")
        assertTrue("the scan waits for the forget", joined >= 0 && joined < rescan.indexOf("onScan()"))
    }
}
