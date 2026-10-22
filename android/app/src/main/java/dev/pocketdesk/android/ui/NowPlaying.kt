package dev.pocketdesk.android.ui

/**
 * What the computer says is playing (T8c), as the Sound sheet's "Now
 * playing" card shows it. Kept apart from Android so it is tested.
 */
data class NowPlaying(
    val title: String,
    val artist: String,
    /** The playing app, as the computer's system names it. */
    val app: String,
    val isPlaying: Boolean,
    val canPlayPause: Boolean,
    val canNext: Boolean,
    val canPrevious: Boolean,
    /** The player's id, as the computer lists its players (O6). */
    val player: String = "",
) {
    /** The title, or the app's name when the player gives none. */
    fun headline(): String = title.trim().ifEmpty { appName(app) }

    /** Who and where: the artist and the app, those there are. */
    fun subline(): String {
        // With no title the app is already the headline.
        val where = if (title.isBlank()) "" else appName(app)
        return listOf(artist.trim(), where).filter { it.isNotEmpty() }.joinToString(" · ")
    }

    companion object {
        /**
         * The app's name from the id the computer's system gives it:
         * "Spotify.exe" is Spotify, a Store app's
         * "Publisher.Name_hash!Publisher.Name" is Name.
         */
        fun appName(id: String): String {
            val name = id.substringAfter('!').trim()
            if (name.endsWith(".exe", ignoreCase = true)) return name.dropLast(4)
            return name.substringAfterLast('.')
        }
    }
}
