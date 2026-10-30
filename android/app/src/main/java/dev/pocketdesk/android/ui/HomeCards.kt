package dev.pocketdesk.android.ui

/**
 * The cards Home shows above the computers. They speak of features that send
 * to a saved computer, so with none saved there are none: a computer found
 * on the network let them through, over the first-run explainer (UX1). And
 * one at a time, Health first: the two together filled most of a phone's
 * screen and pushed the first computer below the fold (UX1).
 */
object HomeCards {
    enum class Card { HEALTH, BACKGROUND }

    fun shown(hasSaved: Boolean, health: Boolean, background: Boolean): Card? = when {
        !hasSaved -> null
        health -> Card.HEALTH
        background -> Card.BACKGROUND
        else -> null
    }
}
