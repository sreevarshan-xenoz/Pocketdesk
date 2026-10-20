package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.RemoteSession.State
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteSessionTest {
    @Test
    fun `the bridge's state is read`() {
        assertEquals("ready", RemoteSession.parse("""{"state":"ready"}"""))
        assertEquals("tooOld", RemoteSession.parse("""{"state": "tooOld"}"""))
        assertEquals("none", RemoteSession.parse("garbage"))
    }

    /** A sound session hears which of the two the computer allows (N5 review). */
    @Test
    fun `a sound session's grants are read`() {
        assertEquals(
            RemoteSession.Grants(audio = true, mic = false),
            RemoteSession.grants("""{"state":"ready","audio":true,"mic":false}"""),
        )
        assertEquals(
            RemoteSession.Grants(audio = false, mic = true),
            RemoteSession.grants("""{"state":"ready","audio": false,"mic": true}"""),
        )
        // Not said: both, as a computer that does not say allows what it allows.
        assertEquals(RemoteSession.Grants(audio = true, mic = true), RemoteSession.grants("""{"state":"ready"}"""))
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
        // The phone closed it for that: the closed reason follows and must not hide why.
        assertEquals(State.TooOld, RemoteSession.next(State.Waiting, "tooOld", "input-only session not known here"))
    }

    /** I1b: a remote takes a long quiet as a computer gone. */
    @Test
    fun `twelve seconds of quiet is a lost link`() {
        assertEquals("an older bridge says nothing", 0L, RemoteSession.quietMs("""{"state":"ready"}"""))
        assertEquals(12_500L, RemoteSession.quietMs("""{"state":"ready","quietMs":12500}"""))
        assertTrue(!RemoteSession.silent(11_999))
        assertTrue(RemoteSession.silent(12_000))
    }

    /** J6: one rule says a ready session is gone quiet, for the remote and the sound session. */
    @Test
    fun `a ready session heard from in twelve seconds is not gone`() {
        val quiet = """{"state":"ready","quietMs":12500}"""
        val heard = """{"state":"ready","quietMs":4000}"""
        assertTrue(RemoteSession.goneQuiet(RemoteSession.State.Ready, null, quiet))
        assertFalse("heard from", RemoteSession.goneQuiet(RemoteSession.State.Ready, null, heard))
        assertFalse("not admitted yet", RemoteSession.goneQuiet(RemoteSession.State.Waiting, null, quiet))
        assertFalse("the transport said why", RemoteSession.goneQuiet(RemoteSession.State.Ready, "timed out", quiet))
        assertFalse("ended already", RemoteSession.goneQuiet(RemoteSession.State.Down, null, quiet))
        assertFalse("too old", RemoteSession.goneQuiet(RemoteSession.State.TooOld, null, quiet))
    }

    /** J6 review: only a session heard from lately counts as healthy. */
    @Test
    fun `a session is heard from within one link report`() {
        assertTrue(RemoteSession.heard("""{"state":"ready","quietMs":4999}"""))
        assertFalse(RemoteSession.heard("""{"state":"ready","quietMs":5000}"""))
        assertTrue("an older bridge says nothing", RemoteSession.heard("""{"state":"ready"}"""))
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
