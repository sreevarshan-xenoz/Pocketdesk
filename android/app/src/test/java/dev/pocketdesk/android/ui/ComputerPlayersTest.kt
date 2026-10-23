package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.ComputerPlayers.Player
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerPlayersTest {
    private val spotify = Player("Spotify.exe", "Spotify.exe", true)
    private val chrome = Player("Chrome", "Chrome", false)

    @Test
    fun `a player is named by a short plain id`() {
        assertTrue(ComputerPlayers.valid("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"))
        assertFalse(ComputerPlayers.valid(null))
        assertFalse(ComputerPlayers.valid(""))
        assertFalse(ComputerPlayers.valid("spotify,%any"))
        assertFalse(ComputerPlayers.valid("a\nb"))
        assertFalse(ComputerPlayers.valid("x".repeat(ComputerPlayers.MAX_ID + 1)))
        // Bytes, as the computer counts them (O6 review).
        assertFalse(ComputerPlayers.valid("é".repeat(ComputerPlayers.MAX_ID / 2 + 1)))
        assertNull(ComputerPlayers.named("a,b"))
        assertEquals("vlc", ComputerPlayers.named("vlc"))
    }

    @Test
    fun `a choice is offered with two players, or one chosen`() {
        assertFalse(ComputerPlayers.offered(emptyList(), null))
        assertFalse(ComputerPlayers.offered(listOf(spotify), null))
        assertTrue(ComputerPlayers.offered(listOf(spotify, chrome), null))
        // One chosen that has gone can still be undone.
        assertTrue(ComputerPlayers.offered(emptyList(), "Chrome"))
    }

    @Test
    fun `the chosen player counts only while it is listed`() {
        assertEquals(chrome, ComputerPlayers.current(listOf(spotify, chrome), "Chrome"))
        assertNull(ComputerPlayers.current(listOf(spotify), "Chrome"))
        assertNull(ComputerPlayers.current(listOf(spotify), null))
    }

    @Test
    fun `two players of one name are told apart by their id`() {
        val a = Player("firefox.instance_1", "firefox", true)
        val b = Player("firefox.instance_2", "firefox", false)
        val short = { name: String -> name }
        assertEquals("firefox (firefox.instance_1)", ComputerPlayers.label(listOf(a, b, spotify), a, short))
        assertEquals("Spotify.exe", ComputerPlayers.label(listOf(a, b, spotify), spotify, short))
    }

    @Test
    fun `a push read for another player is not shown`() {
        assertTrue(ComputerPlayers.pushFor(hasPlayer = false, pushed = null, chosen = "Chrome"))
        assertTrue(ComputerPlayers.pushFor(hasPlayer = true, pushed = "Chrome", chosen = "Chrome"))
        assertTrue(ComputerPlayers.pushFor(hasPlayer = true, pushed = null, chosen = null))
        assertFalse(ComputerPlayers.pushFor(hasPlayer = true, pushed = "Spotify.exe", chosen = "Chrome"))
        assertFalse(ComputerPlayers.pushFor(hasPlayer = true, pushed = null, chosen = "Chrome"))
    }

    @Test
    fun `the list keeps each player once, and only those that may be named`() {
        val many = (0 until ComputerPlayers.MAX_PLAYERS + 4).map { Player("p$it", "p$it", false) }
        assertEquals(ComputerPlayers.MAX_PLAYERS, ComputerPlayers.kept(many).size)
        assertEquals(
            listOf(spotify),
            ComputerPlayers.kept(listOf(spotify, spotify.copy(isPlaying = false), Player("a,b", "x", false))),
        )
    }
}
