package dev.pocketdesk.wear

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Test

/** Every string the watch shows in English is there in Spanish and Tamil, with the same placeholders (as the phone app's test holds). */
class StringsTranslatedTest {
    private val entry = Regex("""<string name="([^"]+)"([^>]*)>(.*?)</string>""", RegexOption.DOT_MATCHES_ALL)
    private val comment = Regex("""<!--.*?-->""", RegexOption.DOT_MATCHES_ALL)
    private val placeholder = Regex("""%\d+\$[sd]""")

    private fun strings(dir: String): Map<String, List<String>> {
        val xml = File("src/main/res/$dir/strings.xml").readText().replace(comment, "")
        return entry.findAll(xml)
            .filterNot { it.groupValues[2].contains("translatable=\"false\"") }
            .associate { m -> m.groupValues[1] to placeholder.findAll(m.groupValues[3]).map { it.value }.distinct().sorted().toList() }
    }

    @Test
    fun everyStringIsInSpanishAndTamil() {
        val english = strings("values")
        assert(english.size > 30) { "too few strings read: ${english.size}" }
        for (language in listOf("values-es", "values-ta")) {
            val translated = strings(language)
            assertEquals("missing in $language", emptySet<String>(), english.keys - translated.keys)
            assertEquals("only in $language", emptySet<String>(), translated.keys - english.keys)
            val mismatched = english.filter { (name, held) -> translated[name] != held }.keys
            assertEquals("placeholders differ in $language", emptySet<String>(), mismatched)
        }
    }

    @Test
    fun spanishSaysEquipoAndEmparejar() {
        val xml = File("src/main/res/values-es/strings.xml").readText().replace(comment, "")
        val found = entry.findAll(xml).filter { Regex("(?i)(ordenador|computadora|vincul)").containsMatchIn(it.groupValues[3]) }.map { it.groupValues[1] }.toList()
        assertEquals("strings not saying equipo or emparejar", emptyList<String>(), found)
    }
}
