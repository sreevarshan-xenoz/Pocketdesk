package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.RemoteSession.State
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteSessionTest {
    @Test
    fun `the bridge's state is read`() {
        assertEquals("ready", RemoteSession.parse("""{"state":"ready"}"""))
        assertEquals("tooOld", RemoteSession.parse("""{"state": "tooOld"}"""))
        assertEquals("none", RemoteSession.parse("garbage"))
    }

    @Test
    fun `a remote waits, is admitted, and stays admitted until it ends`() {
        assertEquals(State.Waiting, RemoteSession.next(State.Connecting, "waiting", null))
        assertEquals(State.Ready, RemoteSession.next(State.Waiting, "ready", null))
        assertEquals(State.Ready, RemoteSession.next(State.Ready, "waiting", null))
        assertEquals(State.Down, RemoteSession.next(State.Ready, "ready", "phone went silent"))
        assertEquals(State.Down, RemoteSession.next(State.Ready, "none", null))
        assertEquals(State.Connecting, RemoteSession.next(State.Connecting, "none", null))
    }

    @Test
    fun `a computer that streams anyway is too old`() {
        assertEquals(State.TooOld, RemoteSession.next(State.Waiting, "tooOld", null))
    }

    @Test
    fun `the rows send keys the bridge maps`() {
        // android/bridge/src/api.rs maps these Android key codes to evdev.
        val mapped = setOf(21, 22, 24, 25, 30, 85, 87, 88, 92, 93, 111, 135)
        (RemoteSession.media + RemoteSession.presenter).forEach {
            assertTrue("${it.label} (${it.keyCode})", it.keyCode in mapped)
        }
    }
}
