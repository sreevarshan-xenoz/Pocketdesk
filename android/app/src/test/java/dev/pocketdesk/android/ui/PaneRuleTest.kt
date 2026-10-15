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
}
