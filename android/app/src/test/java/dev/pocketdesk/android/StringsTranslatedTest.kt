package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Every string the app shows in English is there in Spanish and Tamil, with
 * the same placeholders (U6): lint only warns about a missing translation,
 * and Android then shows that one line in English.
 */
class StringsTranslatedTest {
    private val entry =
        Regex("""<(string|plurals) name="([^"]+)"([^>]*)>(.*?)</\1>""", RegexOption.DOT_MATCHES_ALL)
    private val comment = Regex("""<!--.*?-->""", RegexOption.DOT_MATCHES_ALL)
    private val placeholder = Regex("""%\d+\$[sd]""")

    /**
     * Each translatable string's name and its distinct placeholders, sorted
     * (a language's plurals have their own number of items); comments left out.
     */
    private fun strings(dir: String): Map<String, List<String>> {
        val xml = File("src/main/res/$dir/strings.xml").readText().replace(comment, "")
        return entry.findAll(xml)
            .filterNot { it.groupValues[3].contains("translatable=\"false\"") }
            .associate { m ->
                m.groupValues[2] to placeholder.findAll(m.groupValues[4]).map { it.value }.distinct().sorted().toList()
            }
    }

    @Test
    fun everyStringIsInSpanishAndTamil() {
        val english = strings("values")
        for (language in listOf("values-es", "values-ta")) {
            val translated = strings(language)
            assertEquals("missing in $language", emptySet<String>(), english.keys - translated.keys)
            assertEquals("only in $language", emptySet<String>(), translated.keys - english.keys)
            val mismatched = english.filter { (name, held) -> translated[name] != held }.keys
            assertEquals("placeholders differ in $language", emptySet<String>(), mismatched)
        }
    }

    /**
     * Spanish says "equipo" for a computer everywhere (R5) and "emparejar"
     * for pairing: "ordenador", "computadora" and "vincular" crept back in
     * with later features, more than one word for one thing.
     */
    @Test
    fun spanishSaysEquipoAndEmparejar() {
        val xml = File("src/main/res/values-es/strings.xml").readText().replace(comment, "")
        val strings = entry.findAll(xml).toList()
        // Not passed for reading nothing (PR #585 review N3).
        assertTrue("too few Spanish strings read: ${strings.size}", strings.size > 500)
        assertTrue("no string says equipo", strings.count { it.groupValues[4].contains("equipo") } > 100)
        val other = Regex("""(?i)(ordenador|computadora|vincul)""")
        val found = strings
            .filter { other.containsMatchIn(it.groupValues[4]) }
            .map { it.groupValues[2] }
        assertEquals("strings not saying equipo or emparejar", emptyList<String>(), found)
    }
}
