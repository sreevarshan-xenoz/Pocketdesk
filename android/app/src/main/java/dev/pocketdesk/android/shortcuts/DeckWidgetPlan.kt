package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.Deck

/**
 * The deck widget's choices (G3), kept apart from Android so they are
 * tested: which buttons fill its slots, when to ask for the list, what a
 * press's line says and whose list is kept.
 */
object DeckWidgetPlan {
    /** The buttons a widget holds: two rows of three. */
    const val SLOTS = 6

    /** The first [SLOTS] buttons in the computer's order; an empty slot is null. */
    fun slots(buttons: List<Deck.Button>): List<Deck.Button?> =
        List(SLOTS) { buttons.getOrNull(it) }

    /** Why the widget is being drawn. */
    enum class Why { Refresh, Follow, Period }

    /**
     * Whether to ask the computer for its buttons: for ↻, or when the last
     * computer was never [tried] (a widget just placed, a new last computer).
     * A deck changes when its owner edits it, so a period never asks again.
     */
    fun askList(why: Why, tried: Boolean): Boolean = why == Why.Refresh || !tried

    /** What the line says of a press. */
    enum class Said { Done, Waiting, Refused, Failed }

    /** A press's [outcome], or null when the computer was not reached. */
    fun said(outcome: Deck.Outcome?): Said = when {
        outcome == null -> Said.Failed
        outcome.waiting -> Said.Waiting
        outcome.ok -> Said.Done
        else -> Said.Refused
    }

    /** A kept list is shown only while its computer is still the last one. */
    fun keeps(listFor: String?, lastComputer: String?): Boolean =
        listFor != null && listFor == lastComputer

    /**
     * Whose list a widget keeps, and whose buttons it draws: a paired
     * computer by its paired id, so its list follows a move and another
     * computer saved at its old address never shows or presses it (AZ2: by
     * address it did both); one never paired by its address.
     */
    fun computerKey(address: String, pairedId: String?): String = pairedId?.let { "paired:$it" } ?: address

    /** Whether a kept list goes: another computer's, or any with none left (AZ2). */
    fun forgets(listFor: String?, lastComputer: String?): Boolean =
        listFor != null && !keeps(listFor, lastComputer)
}
