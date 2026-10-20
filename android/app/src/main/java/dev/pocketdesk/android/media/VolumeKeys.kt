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

    /** The step one press in [direction] (-1 down, +1 up, 0 none) moves to from [step]. */
    fun adjust(step: Int, direction: Int): Int = (step + direction.coerceIn(-1, 1)).coerceIn(0, STEPS)
}
