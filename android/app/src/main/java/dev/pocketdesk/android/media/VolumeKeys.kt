package dev.pocketdesk.android.media

/**
 * The computer's volume as the phone's volume keys move it (K4): Android's
 * panel shows [STEPS] steps of 5 %, a press moves one. Pure, for JVM tests.
 */
object VolumeKeys {
    const val STEPS = 20

    /** The panel's step for the computer's [percent] (0 to 100). */
    fun toStep(percent: Int): Int = ((percent.coerceIn(0, 100) + 2) / 5).coerceIn(0, STEPS)

    /** The computer's percent for a panel [step]. */
    fun toPercent(step: Int): Int = step.coerceIn(0, STEPS) * 5

    /**
     * Whether the computer takes the keys with the phone's audio in [mode]:
     * not in a call or a call app's own audio (K4 review), so the call's
     * volume stays the keys'.
     */
    // The newer modes' constants are compiled in: an older phone never reports them.
    @android.annotation.SuppressLint("InlinedApi")
    fun takesKeys(mode: Int): Boolean =
        mode !in setOf(
            android.media.AudioManager.MODE_IN_CALL,
            android.media.AudioManager.MODE_IN_COMMUNICATION,
            android.media.AudioManager.MODE_CALL_SCREENING,
            android.media.AudioManager.MODE_CALL_REDIRECT,
            android.media.AudioManager.MODE_COMMUNICATION_REDIRECT,
        )

    /** How long the computer's volume as last read stands for it, in ms. */
    const val FRESH_MS = 3_000L

    /** Whether a reading at [readAt] is fresh at [now] (both elapsedRealtime). */
    fun fresh(readAt: Long, now: Long): Boolean = readAt > 0 && now - readAt in 0 until FRESH_MS

    /** The step [presses] (summed, up positive) move to from [base]. */
    fun target(base: Int, presses: Int): Int = (base + presses).coerceIn(0, STEPS)

    /** The step one press in [direction] (-1 down, +1 up, 0 none) moves to from [step]. */
    fun adjust(step: Int, direction: Int): Int = (step + direction.coerceIn(-1, 1)).coerceIn(0, STEPS)
}
