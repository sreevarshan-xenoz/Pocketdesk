package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * C5: the session menu's choices at the owner's font scale, on a device: as
 * segments on one line while their words fit, as radio buttons one per line
 * once they do not (at 2.0 the Quality row had broken "Balanced" and
 * "Economy" mid-word).
 */
@RunWith(AndroidJUnit4::class)
class ChoiceRowTest {
    @get:Rule
    val compose = createComposeRule()

    private val radio = SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton)

    private fun show(fontScale: Float) {
        compose.setContent {
            val base = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(base.density, fontScale)) {
                // The sheet's width on a phone held upright.
                Box(Modifier.width(400.dp)) {
                    ChoiceRow("Quality", listOf("Auto", "Max", "Balanced", "Economy"), selected = 0, onSelect = {})
                }
            }
        }
    }

    /** The tops of the four choices (segments and list rows are both radio buttons to TalkBack). */
    private fun tops(): List<Float> {
        compose.onAllNodes(radio).assertCountEquals(4)
        return compose.onAllNodes(radio).fetchSemanticsNodes().map { it.boundsInRoot.top }
    }

    @Test
    fun atTheUsualSizeTheChoicesAreSegmentsOnOneLine() {
        show(1f)
        assertEquals("one line", 1, tops().distinct().size)
    }

    @Test
    fun atTwiceTheSizeTheyAreRadioButtonsOnePerLine() {
        show(2f)
        assertEquals("a line each", 4, tops().distinct().size)
    }
}
