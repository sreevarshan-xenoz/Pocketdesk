package dev.pocketdesk.android.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SessionTellTest {
    @Test
    fun aSessionStartsAndEndsOnceEach() {
        val tell = SessionTell("screen")
        assertNull("not up yet", tell.left())
        val started = tell.up()!!
        assertEquals(PhoneEvents.Kind.SESSION_STARTED, started.kind)
        assertEquals("screen", started.fields["mode"])
        assertNull("already up", tell.up())
        val lost = tell.lost()!!
        assertEquals(PhoneEvents.Kind.SESSION_ENDED, lost.kind)
        assertEquals("lost", lost.fields["reason"])
        assertNull("already ended", tell.left())
    }

    @Test
    fun aReconnectIsANewSessionAndLeavingEndsIt() {
        val tell = SessionTell("remote")
        tell.up()
        tell.lost()
        assertEquals(PhoneEvents.Kind.SESSION_STARTED, tell.up()!!.kind)
        assertEquals("left", tell.left()!!.fields["reason"])
    }
}
