package dev.pocketdesk.android.ui

/**
 * O6: the computer's players, as its `media` answer lists them, and the one
 * this phone chose for that computer. A chosen player is named on every
 * media request (the Sound sheet, the widget, N5's watch); with none, or
 * one that has gone, the computer's own choice stands. Pure, for JVM tests.
 */
object ComputerPlayers {
    /** One player: its [id] as the computer names it, the [name] it goes by, whether it plays. */
    data class Player(val id: String, val name: String, val isPlaying: Boolean)

    /** The most players shown, as the computer lists at most. */
    const val MAX_PLAYERS = 16

    /** The longest id a player may have. */
    const val MAX_ID = 256

    /** Whether [id] may name a player: as the computer's own rule, at most [MAX_ID] bytes, plain, no comma. */
    fun valid(id: String?): Boolean =
        id != null && id.isNotEmpty() && id.toByteArray(Charsets.UTF_8).size <= MAX_ID &&
            id.none { it.isISOControl() || it == ',' }

    /**
     * What [p] is called in the list: its [name] through [shortName], with
     * its id beside it when another player is called the same (two windows
     * of one browser, O6 review).
     */
    fun label(players: List<Player>, p: Player, shortName: (String) -> String): String {
        val name = shortName(p.name)
        val shared = players.count { shortName(it.name) == name } > 1
        return if (shared) "$name (${p.id})" else name
    }

    /**
     * Whether a push read for [pushed] (null: the computer's choice; absent
     * from an older computer, [hasPlayer] false) is for the player [chosen]
     * now: one read before a new choice is not shown (O6 review).
     */
    fun pushFor(hasPlayer: Boolean, pushed: String?, chosen: String?): Boolean =
        !hasPlayer || named(pushed) == named(chosen)

    /** The id to name on a request: [chosen] when it may name one, else none. */
    fun named(chosen: String?): String? = chosen?.takeIf { valid(it) }

    /** Whether to offer a choice: two players or more, or one chosen already, so it can be undone. */
    fun offered(players: List<Player>, chosen: String?): Boolean = players.size > 1 || named(chosen) != null

    /** The player [chosen] names, while the computer still lists it; else null (the computer's choice). */
    fun current(players: List<Player>, chosen: String?): Player? = named(chosen)?.let { id -> players.firstOrNull { it.id == id } }

    /** [players] as listed, each once, at most [MAX_PLAYERS], none with an id that may not name one. */
    fun kept(players: List<Player>): List<Player> =
        players.filter { valid(it.id) }.distinctBy { it.id }.take(MAX_PLAYERS)
}
