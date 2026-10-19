package dev.pocketdesk.android.ui

/**
 * How "Pane: auto" splits the session, and how much the stream gets. Kept
 * apart from Android so it is tested.
 */
object PaneRule {
    /** A window this many dp wide is a tablet or an unfolded phone. */
    const val WIDE_DP = 600f

    /** A window this much taller than wide is a phone held upright. */
    const val PORTRAIT_RATIO = 1.25f

    /**
     * The least and most of a portrait screen the stream pane takes: the
     * trackpad needs no more than 45 % of it, and keeps at least 30 %.
     */
    const val MIN_PORTRAIT_STREAM = 0.55f
    const val MAX_PORTRAIT_STREAM = 0.7f

    /**
     * The room an upright stream pane keeps above and below the picture for
     * its chrome (the link, the details and window chips on top; the mode,
     * keyboard and menu below), so none of it sits on the desktop.
     */
    const val TOP_BAND_DP = 148f
    const val BOTTOM_BAND_DP = 76f

    /**
     * The split for [fold] (a hinge, or none) in a window [widthDp] by
     * [heightDp] whose smallest width is [smallestWidthDp] (the window's
     * `smallestScreenWidthDp`, the sw600dp rule): the
     * hinge's own axis; side by side when the window is wide on a tablet or
     * an unfolded phone; the stream on top and the controls below on a phone
     * held upright (T9b); else one pane, a phone on its side included (C5:
     * it is wide, and the split gave its stream half the screen).
     */
    fun autoSplit(fold: PaneSplit, widthDp: Float, heightDp: Float, smallestWidthDp: Float = WIDE_DP): PaneSplit = when {
        fold != PaneSplit.None -> fold
        widthDp >= WIDE_DP && smallestWidthDp >= WIDE_DP -> PaneSplit.Vertical
        heightDp >= widthDp * PORTRAIT_RATIO -> PaneSplit.Horizontal
        else -> PaneSplit.None
    }

    /**
     * The split for the Pane [mode] in a window [widthDp] by [heightDp]
     * (measured without the on-screen keyboard), [fold] a hinge or none:
     * Auto as [autoSplit]; Dual two panes, stacked when upright (as Auto
     * there) and side by side otherwise; Single one. Picture-in-picture shows
     * the stream alone. When the input [touchesStream] (direct touch, the
     * pen), Auto keeps an upright phone to one pane: a trackpad under the
     * picture would drive nothing, and the picture gets the whole screen.
     */
    fun resolve(
        mode: PaneMode,
        fold: PaneSplit,
        widthDp: Float,
        heightDp: Float,
        pip: Boolean,
        smallestWidthDp: Float = WIDE_DP,
        touchesStream: Boolean = false,
    ): PaneSplit {
        if (pip) return PaneSplit.None
        val auto = autoSplit(fold, widthDp, heightDp, smallestWidthDp)
        return when (mode) {
            PaneMode.Auto ->
                if (auto == PaneSplit.Horizontal && fold == PaneSplit.None && touchesStream) PaneSplit.None else auto
            PaneMode.Dual -> if (auto != PaneSplit.None) auto else PaneSplit.Vertical
            PaneMode.Single -> PaneSplit.None
        }
    }

    /**
     * Whether the session's chrome stays rather than fading: beside a
     * side-by-side dock or a hinge's, a control surface of its own; and on
     * an upright phone whose picture keeps bands for it ([banded]), where it
     * covers nothing, so fading it would hide the controls for nothing and
     * bringing it back by tapping a trackpad's picture would click.
     */
    fun chromeStays(split: PaneSplit, hinge: Boolean, banded: Boolean = false): Boolean =
        banded || split == PaneSplit.Vertical || (split == PaneSplit.Horizontal && hinge)

    /**
     * Whether the keyboard bar, along the window's bottom, covers the stream
     * in a [split] (it then needs padding): not when the stream is on top.
     */
    fun streamUnderKeyboardBar(split: PaneSplit): Boolean = split != PaneSplit.Horizontal

    /**
     * The share of the window's height the stream pane takes in a [split]
     * ([hinge] when a fold made it): on a phone held upright, as tall as a
     * desktop of [videoAspect] (width / height) is at full width plus
     * [bandsPx] of room for the chrome, within [MIN_PORTRAIT_STREAM] and
     * [MAX_PORTRAIT_STREAM]; otherwise half.
     * While the session is not [up], the upright stream pane takes the whole
     * screen: its card (the reason, Retry) does not fit a quarter of it, and
     * the dock below has nothing to drive meanwhile.
     */
    fun streamFraction(
        split: PaneSplit,
        hinge: Boolean,
        widthPx: Float,
        heightPx: Float,
        videoAspect: Float,
        up: Boolean = true,
        bandsPx: Float = 0f,
    ): Float {
        if (split == PaneSplit.Horizontal && !hinge && !up) return 1f
        if (split != PaneSplit.Horizontal || hinge || videoAspect <= 0f || widthPx <= 0f || heightPx <= 0f) return 0.5f
        return ((widthPx / videoAspect + bandsPx) / heightPx).coerceIn(MIN_PORTRAIT_STREAM, MAX_PORTRAIT_STREAM)
    }
}
