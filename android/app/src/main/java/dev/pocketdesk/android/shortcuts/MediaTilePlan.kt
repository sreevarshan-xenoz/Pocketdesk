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

    /** A reading older than this is not shown as what plays now (G6 review). */
    const val FRESH_MS = 10 * 60 * 1000L

    fun look(
        hasComputer: Boolean,
        playing: NowPlaying?,
        failed: Boolean,
        asking: Boolean,
        ageMs: Long = 0L,
    ): Look = when {
        !hasComputer -> Look.NoComputer
        asking -> Look.Asking
        failed -> Look.Failed
        playing == null || ageMs > FRESH_MS -> Look.Unknown
        playing.isPlaying -> Look.Playing
        else -> Look.Paused
    }
}
