package dev.pocketdesk.android.ui

import java.text.Normalizer

/**
 * O8: which of Settings' rows a search shows, as J2's rule does on the
 * desktop: a row whose title, summary and group's name hold every word
 * typed, in any case and any order. A word typed matches the start of a
 * word there ("ring" finds "Ring", not "during"), accents aside ("conexion"
 * finds "conexión", O8 review). Pure, for JVM tests.
 */
object SettingsSearch {
    private val NOT_A_WORD = Regex("[^\\p{L}\\p{N}\\p{M}]+")
    private val MARKS = Regex("\\p{Mn}+")

    /** [text] lower-cased with its accents taken off, split into words. */
    private fun wordsOf(text: String): List<String> =
        Normalizer.normalize(text.lowercase(), Normalizer.Form.NFD)
            .replace(MARKS, "")
            .split(NOT_A_WORD)
            .filter { it.isNotEmpty() }

    /** The words typed; none for a blank search. */
    fun words(query: String): List<String> = wordsOf(query)

    /** Whether anything is being looked for. */
    fun searching(query: String): Boolean = words(query).isNotEmpty()

    /** Whether a row of [texts] in [group] shows for [query]: always with nothing typed. */
    fun rowShown(query: String, texts: List<String>, group: String): Boolean {
        val typed = words(query)
        if (typed.isEmpty()) return true
        val there = (texts + group).flatMap { wordsOf(it) }
        return typed.all { word -> there.any { it.startsWith(word) } }
    }

    /** Whether [group]'s heading shows: nothing typed, or one of its rows found. */
    fun groupShown(query: String, rowsFound: Int): Boolean = !searching(query) || rowsFound > 0
}
