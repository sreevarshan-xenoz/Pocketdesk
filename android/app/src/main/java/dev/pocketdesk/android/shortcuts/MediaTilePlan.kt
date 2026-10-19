package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.NowPlaying

/**
 * The music tile's look (G6), kept apart from Android so it is tested:
 * on while the computer plays, and what its subtitle says.
 */
object MediaTilePlan {
    /** What the tile shows; [on] when it is drawn active. */
    enum class Look(val on: Boolean) {
        NoComputer(false),
        Asking(false),
        Failed(false),
        Unknown(false),
        Paused(false),
        Playing(true),
    }

    fun look(hasComputer: Boolean, playing: NowPlaying?, failed: Boolean, asking: Boolean): Look = when {
        !hasComputer -> Look.NoComputer
        asking -> Look.Asking
        failed -> Look.Failed
        playing == null -> Look.Unknown
        playing.isPlaying -> Look.Playing
        else -> Look.Paused
    }
}
