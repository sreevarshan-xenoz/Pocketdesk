package dev.pocketdesk.android.ui

/**
 * The session chrome's text size (C5): the owner's font scale, up to 1.3.
 * The chrome is a few words over moving video, in a pane that can be a
 * quarter of the screen; at 2.0 its chips ran into each other. The session
 * menu, its sheets and every other screen keep the full scale. Kept apart
 * from Android so it is tested.
 */
object ChromeText {
    /** The largest font scale the chrome draws at. */
    const val MAX_SCALE = 1.3f

    /** The chrome's font scale for the owner's [fontScale]. */
    fun scale(fontScale: Float): Float = fontScale.coerceAtMost(MAX_SCALE)
}
