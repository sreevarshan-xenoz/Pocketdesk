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

    /** The least and most of a portrait screen the stream pane takes. */
    const val MIN_PORTRAIT_STREAM = 0.25f
    const val MAX_PORTRAIT_STREAM = 0.6f

    /**
     * The split for [fold] (a hinge, or none) in a window [widthDp] by
     * [heightDp]: the hinge's own axis; side by side when wide; the stream
     * on top and the controls below on a phone held upright (T9b); else one
     * pane.
     */
    fun autoSplit(fold: PaneSplit, widthDp: Float, heightDp: Float): PaneSplit = when {
        fold != PaneSplit.None -> fold
        widthDp >= WIDE_DP -> PaneSplit.Vertical
        heightDp >= widthDp * PORTRAIT_RATIO -> PaneSplit.Horizontal
        else -> PaneSplit.None
    }

    /**
     * The share of the window's height the stream pane takes in a [split]
     * ([hinge] when a fold made it): on a phone held upright, as tall as a
     * desktop of [videoAspect] (width / height) is at full width, within
     * [MIN_PORTRAIT_STREAM] and [MAX_PORTRAIT_STREAM]; otherwise half.
     */
    fun streamFraction(
        split: PaneSplit,
        hinge: Boolean,
        widthPx: Float,
        heightPx: Float,
        videoAspect: Float,
    ): Float {
        if (split != PaneSplit.Horizontal || hinge || videoAspect <= 0f || heightPx <= 0f) return 0.5f
        return (widthPx / videoAspect / heightPx).coerceIn(MIN_PORTRAIT_STREAM, MAX_PORTRAIT_STREAM)
    }
}
