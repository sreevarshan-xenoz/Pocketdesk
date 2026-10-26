package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
    }
}
