package dev.pocketdesk.android.ui

/**
 * The sides of a computer card's buttons (UX1): 12 dp when the row fits one
 * line so, 8 dp when only that fits, and 12 dp again when neither does (the
 * row then wraps, and roomier buttons read better on two lines). Spanish's
 * "Conectar / Mando / Mostrar pantalla" wrapped at 12 dp on a 360 dp phone
 * (PR #575 review N7). Kept apart from Android so it is tested.
 */
object CardButtonFit {
    const val ROOMY_DP = 12
    const val TIGHT_DP = 8

    /** Material's narrowest button. */
    private const val MIN_BUTTON_DP = 58f

    /**
     * Room kept spare: a measured width a fraction of a dp under the row's
     * on a boundary could pick a fit that wraps (UX2, review N12).
     */
    const val SLACK_DP = 1f

    fun sideDp(labelWidthsDp: List<Float>, rowWidthDp: Float, gapDp: Float = 8f): Int {
        fun width(side: Int) = labelWidthsDp.sumOf { maxOf(it + 2 * side, MIN_BUTTON_DP).toDouble() } +
            gapDp * (labelWidthsDp.size - 1)
        return when {
            width(ROOMY_DP) <= rowWidthDp - SLACK_DP -> ROOMY_DP
            width(TIGHT_DP) <= rowWidthDp - SLACK_DP -> TIGHT_DP
            else -> ROOMY_DP
        }
    }
}
