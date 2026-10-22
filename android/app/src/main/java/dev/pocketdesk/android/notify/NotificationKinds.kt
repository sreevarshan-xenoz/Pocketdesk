package dev.pocketdesk.android.notify

/**
 * O5: the kinds of notification a shared app posts (its Android
 * notification channels), so each kind can stay on the phone while the
 * app's others go to the computer: a chat app's "Updating messages…"
 * without its messages. A listener cannot read another app's channel names
 * (Android keeps them for companion apps), so a kind is known by its id and
 * the title of the last notification seen in it. Pure, for JVM tests.
 */
object NotificationKinds {
    /** How many kinds an app keeps; the least recently seen goes first. */
    const val MAX_KINDS = 20

    /** The longest example kept, in characters. */
    const val MAX_EXAMPLE = 48

    /** One kind: its channel [id], and the [example] title last seen in it. */
    data class Kind(val id: String, val example: String)

    /** [title] cut to [MAX_EXAMPLE] characters on one line, as a kind's example. */
    fun example(title: String?): String {
        val line = title.orEmpty().lineSequence().firstOrNull().orEmpty().trim()
        return if (line.length <= MAX_EXAMPLE) line else line.take(MAX_EXAMPLE - 1).trimEnd() + "…"
    }

    /**
     * [kinds] with [id] seen now, with [example] when it has one: moved to
     * the front, at most [MAX_KINDS]. Null when nothing changed (the same
     * kind first with the same example), or [id] is blank.
     */
    fun seen(kinds: List<Kind>, id: String, example: String): List<Kind>? {
        if (id.isBlank()) return null
        val known = kinds.firstOrNull { it.id == id }
        val kept = Kind(id, example.ifBlank { known?.example.orEmpty() })
        if (kinds.firstOrNull() == kept) return null
        return (listOf(kept) + kinds.filter { it.id != id }).take(MAX_KINDS)
    }
}
