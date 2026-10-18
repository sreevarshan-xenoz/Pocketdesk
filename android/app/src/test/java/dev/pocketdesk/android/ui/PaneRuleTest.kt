package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class PaneRuleTest {
    @Test
    fun aHingeOrAWideWindowSplitsAsBeforeAndAPortraitPhoneSplitsTopAndBottom() {
        // A hinge decides its own axis.
        assertEquals(PaneSplit.Horizontal, PaneRule.autoSplit(PaneSplit.Horizontal, 400f, 800f))
        // A tablet or an unfolded phone: side by side.
        assertEquals(PaneSplit.Vertical, PaneRule.autoSplit(PaneSplit.None, 700f, 1000f))
        // A phone held upright: the stream on top, the controls below (T9b).
        assertEquals(PaneSplit.Horizontal, PaneRule.autoSplit(PaneSplit.None, 400f, 860f))
        // A narrow window on its side, or a squarish one: one pane.
        assertEquals(PaneSplit.None, PaneRule.autoSplit(PaneSplit.None, 560f, 400f))
        assertEquals(PaneSplit.None, PaneRule.autoSplit(PaneSplit.None, 500f, 560f))
    }

    @Test
    fun aPortraitStreamPaneIsAsTallAsTheVideoNeeds() {
        // 1080 px wide, 2400 tall; a 16:9 desktop is 607.5 px tall at full width.
        assertEquals(607.5f / 2400f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 1080f, 2400f, 16f / 9f), 1e-4f)
        // A 21:9 one is shorter, but the pane keeps a quarter of the screen.
        assertEquals(0.25f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 1080f, 2400f, 2560f / 1080f), 1e-4f)
        // A tall desktop takes no more than 60 %, so the controls keep room.
        assertEquals(0.6f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 1080f, 2400f, 0.75f), 1e-4f)
        // No video yet, a hinge (it sits in the middle), or side by side: half.
        assertEquals(0.5f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 1080f, 2400f, 0f), 1e-4f)
        assertEquals(0.5f, PaneRule.streamFraction(PaneSplit.Horizontal, true, 1080f, 2400f, 16f / 9f), 1e-4f)
        assertEquals(0.5f, PaneRule.streamFraction(PaneSplit.Vertical, false, 1600f, 2400f, 16f / 9f), 1e-4f)
    }

    @Test
    fun theModeAndTheWindowGiveTheSplitAndPictureInPictureIsOnePane() {
        val portrait = 400f to 860f
        val wideLandscape = 860f to 400f
        val narrowLandscape = 560f to 400f
        fun split(mode: PaneMode, size: Pair<Float, Float>, pip: Boolean = false, fold: PaneSplit = PaneSplit.None) =
            PaneRule.resolve(mode, fold, size.first, size.second, pip)
        assertEquals(PaneSplit.Horizontal, split(PaneMode.Auto, portrait))
        assertEquals(PaneSplit.Vertical, split(PaneMode.Auto, wideLandscape))
        assertEquals(PaneSplit.None, split(PaneMode.Auto, narrowLandscape))
        // Dual asks for two panes: stacked when upright, else side by side.
        assertEquals(PaneSplit.Horizontal, split(PaneMode.Dual, portrait))
        assertEquals(PaneSplit.Vertical, split(PaneMode.Dual, narrowLandscape))
        assertEquals(PaneSplit.None, split(PaneMode.Single, portrait))
        // Picture-in-picture shows the stream alone, whatever its shape.
        assertEquals(PaneSplit.None, split(PaneMode.Auto, portrait, pip = true))
        assertEquals(PaneSplit.None, split(PaneMode.Dual, wideLandscape, pip = true))
        // A hinge keeps its axis.
        assertEquals(PaneSplit.Vertical, split(PaneMode.Auto, portrait, fold = PaneSplit.Vertical))
    }

    @Test
    fun aPhoneOnItsSideKeepsOnePaneAndATabletSplits() {
        // A phone held sideways is wide, but its smallest width is a phone's
        // (C5): the stream keeps the whole screen.
        assertEquals(PaneSplit.None, PaneRule.autoSplit(PaneSplit.None, 914f, 411f, smallestWidthDp = 411f))
        assertEquals(PaneSplit.None, PaneRule.resolve(PaneMode.Auto, PaneSplit.None, 914f, 411f, pip = false, smallestWidthDp = 411f))
        // Dual still splits it on request.
        assertEquals(PaneSplit.Vertical, PaneRule.resolve(PaneMode.Dual, PaneSplit.None, 914f, 411f, pip = false, smallestWidthDp = 411f))
        // Upright, it splits top and bottom as before.
        assertEquals(PaneSplit.Horizontal, PaneRule.autoSplit(PaneSplit.None, 411f, 914f, smallestWidthDp = 411f))
        // A tablet or an unfolded phone, either way up: side by side.
        assertEquals(PaneSplit.Vertical, PaneRule.autoSplit(PaneSplit.None, 1280f, 800f, smallestWidthDp = 800f))
        assertEquals(PaneSplit.Vertical, PaneRule.autoSplit(PaneSplit.None, 673f, 841f, smallestWidthDp = 673f))
        // A hinge still decides.
        assertEquals(PaneSplit.Vertical, PaneRule.autoSplit(PaneSplit.Vertical, 914f, 411f, smallestWidthDp = 411f))
    }

    @Test
    fun theBoundariesFavourTheOldLayouts() {
        // Exactly 600 dp wide is wide, even upright: side by side, as before.
        assertEquals(PaneSplit.Vertical, PaneRule.autoSplit(PaneSplit.None, 600f, 1000f))
        assertEquals(PaneSplit.Horizontal, PaneRule.autoSplit(PaneSplit.None, 599.9f, 1000f))
        // Exactly 1.25 times as tall is upright.
        assertEquals(PaneSplit.Horizontal, PaneRule.autoSplit(PaneSplit.None, 400f, 500f))
        assertEquals(PaneSplit.None, PaneRule.autoSplit(PaneSplit.None, 400f, 499f))
        // Nothing to size against: half.
        assertEquals(0.5f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 0f, 2400f, 16f / 9f), 1e-4f)
        assertEquals(0.5f, PaneRule.streamFraction(PaneSplit.Horizontal, false, 1080f, 2400f, -1f), 1e-4f)
    }

    @Test
    fun onlyTheUprightSplitFadesItsChromeAndKeepsTheStreamClearOfTheKeyboardBar() {
        // The upright split's stream sits on top: the bar covers the dock.
        assertEquals(false, PaneRule.chromeStays(PaneSplit.Horizontal, hinge = false))
        assertEquals(true, PaneRule.chromeStays(PaneSplit.Vertical, hinge = false))
        assertEquals(true, PaneRule.chromeStays(PaneSplit.Horizontal, hinge = true))
        assertEquals(false, PaneRule.chromeStays(PaneSplit.None, hinge = false))
        assertEquals(false, PaneRule.streamUnderKeyboardBar(PaneSplit.Horizontal))
        assertEquals(true, PaneRule.streamUnderKeyboardBar(PaneSplit.Vertical))
        assertEquals(true, PaneRule.streamUnderKeyboardBar(PaneSplit.None))
    }
}
