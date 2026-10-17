package dev.pocketdesk.android

import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

/**
 * Every string the app shows in English is there in Spanish and Tamil (U6):
 * lint only warns about a missing translation, and Android then shows that
 * one line in English.
 */
class StringsTranslatedTest {
    private fun names(dir: String): Set<String> {
        val xml = File("src/main/res/$dir/strings.xml").readText()
        return Regex("""<(?:string|plurals) name="([^"]+)"(?![^>]*translatable="false")""")
            .findAll(xml)
            .map { it.groupValues[1] }
            .toSet()
    }

    @Test
    fun everyStringIsInSpanishAndTamil() {
        val english = names("values")
        for (language in listOf("values-es", "values-ta")) {
            val translated = names(language)
            assertEquals("missing in $language", emptySet<String>(), english - translated)
            assertEquals("only in $language", emptySet<String>(), translated - english)
        }
    }
}
