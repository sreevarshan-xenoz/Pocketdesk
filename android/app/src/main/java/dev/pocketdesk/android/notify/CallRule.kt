package dev.pocketdesk.android.notify

/**
 * A ringing call on the computer (F5): the phone tells its computer of it
 * as one of its notifications, with "Silence" and, when the phone may end
 * calls, "Decline" as its buttons; the computer's answer comes back as a
 * notification's answer. Kept apart from Android so it is tested.
 */
object CallRule {
    /** The ids of call notifications, never an Android notification's key. */
    const val ID_PREFIX = "pocketdesk:call:"

    const val SILENCE = "Silence"
    const val DECLINE = "Decline"

    /** What the computer is shown of a ringing call. */
    data class Note(val id: String, val title: String, val text: String, val actions: List<String>)

    /** The note for call [serial] from [number] ([name] when the contacts know it). */
    fun note(serial: Long, number: String?, name: String?, mayDecline: Boolean): Note {
        val shownNumber = number?.takeIf { it.isNotBlank() }
        val who = when {
            !name.isNullOrBlank() && shownNumber != null -> "$name ($shownNumber)"
            !name.isNullOrBlank() -> name
            shownNumber != null -> shownNumber
            else -> "an unknown number"
        }
        return Note(
            id = ID_PREFIX + serial,
            title = "Call from $who",
            text = "Ringing on the phone",
            actions = if (mayDecline) listOf(SILENCE, DECLINE) else listOf(SILENCE),
        )
    }

    /** Whether a notification answer is for a call note. */
    fun isCall(key: String): Boolean = key.startsWith(ID_PREFIX)

    /** What the phone does with an answer to a call note. */
    enum class Act { Silence, Decline, Gone, NotOffered, NotOwnComputer, Off }

    /**
     * The act for an answer of [action] to [key] from the phone's own
     * computer or not, while the call [ringing] (its id, or null) rings, the
     * calls switch on or off, ending calls allowed or not.
     */
    fun decide(
        key: String,
        action: String?,
        ringing: String?,
        ownComputer: Boolean,
        on: Boolean,
        mayDecline: Boolean,
    ): Act = when {
        !ownComputer -> Act.NotOwnComputer
        !on -> Act.Off
        ringing == null || ringing != key -> Act.Gone
        action == SILENCE -> Act.Silence
        action == DECLINE && mayDecline -> Act.Decline
        else -> Act.NotOffered
    }

    /** Why an act was not done, said to the computer (E1). */
    fun refusal(act: Act): String? = when (act) {
        Act.Gone -> "the call has stopped ringing"
        Act.NotOffered -> "that is not something this phone lets the computer do to its calls"
        Act.NotOwnComputer -> null
        Act.Off -> "calls on the computer are turned off on the phone"
        Act.Silence, Act.Decline -> null
    }
}
