package dev.pocketdesk.android.cast

/**
 * The phone's screen on the computer (T10k): the choices a cast makes, pure
 * for JVM tests.
 */
object CastPlan {
    /** The encoded picture's long side at most: a 1080p-class phone at 0.8×. */
    const val MAX_LONG_SIDE = 1920

    /** About 6 Mbit/s: sharp text at 30 frames a second on a home network. */
    const val BIT_RATE = 6_000_000

    const val FRAME_RATE = 30

    /** A keyframe at least this often, so a lost one heals on its own. */
    const val KEYFRAME_INTERVAL_S = 5

    /** A still screen is sent again this often, so the computer keeps a picture. */
    const val REPEAT_AFTER_US = 100_000L

    /** What the bridge says of a queued config or frame. */
    const val QUEUED = 0
    const val FULL = 1
    const val NO_CAST = 2

    /**
     * The encoded size for a screen of [width]×[height]: the same shape, the
     * long side at most [maxLongSide], both sides multiples of 16 (what every
     * hardware encoder takes).
     */
    fun encodeSize(width: Int, height: Int, maxLongSide: Int = MAX_LONG_SIDE): Pair<Int, Int> {
        val long = maxOf(width, height, 1)
        val scale = if (long > maxLongSide) maxLongSide.toDouble() / long else 1.0
        fun sixteen(side: Int) = maxOf(16, (side * scale / 16).toInt() * 16)
        return sixteen(width) to sixteen(height)
    }

    /** Whether a frame's fate means the phone should send a keyframe now. */
    fun wantsKeyframe(result: Int): Boolean = result == FULL
}
