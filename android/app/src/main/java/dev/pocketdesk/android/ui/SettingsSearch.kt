package dev.pocketdesk.android.ui

/**
 * O8: which of Settings' rows a search shows, as J2's rule does on the
 * desktop: a row whose title, summary and group's name hold every word
 * typed, in any case and any order. Pure, for JVM tests.
 */
object SettingsSearch {
    private val SPACES = Regex("\\s+")

    /** The words typed, lower-cased; none for a blank search. */
    fun words(query: String): List<String> = query.trim().lowercase().split(SPACES).filter { it.isNotEmpty() }

    /** Whether anything is being looked for. */
    fun searching(query: String): Boolean = words(query).isNotEmpty()

    /** Whether a row of [texts] in [group] shows for [query]: always with nothing typed. */
    fun rowShown(query: String, texts: List<String>, group: String): Boolean {
        val words = words(query)
        if (words.isEmpty()) return true
        val haystack = (texts + group).joinToString("\n").lowercase()
        return words.all { it in haystack }
    }

    /** Whether [group]'s heading shows: nothing typed, or one of its rows found. */
    fun groupShown(query: String, rowsFound: Int): Boolean = !searching(query) || rowsFound > 0
}
