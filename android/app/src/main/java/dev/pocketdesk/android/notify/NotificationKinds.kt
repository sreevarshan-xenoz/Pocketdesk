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
    /** How many kinds an app keeps; the oldest not kept on the phone goes first. */
    const val MAX_KINDS = 20

    /** The longest example kept, in characters. */
    const val MAX_EXAMPLE = 48

    /** One kind: its channel [id], and the [example] title first seen in it. */
    data class Kind(val id: String, val example: String)

    /** [title] cut to [MAX_EXAMPLE] characters on one line, as a kind's example. */
    fun example(title: String?): String {
        val line = title.orEmpty().lineSequence().firstOrNull().orEmpty().trim()
        return if (line.length <= MAX_EXAMPLE) line else line.take(MAX_EXAMPLE - 1).trimEnd() + "…"
    }

    /**
     * [kinds] with [id] seen now: a new kind goes first with [example]; a
     * known one stays where it is and takes [example] only if it had none,
     * so an app's every notification is not a write (O5 review). Past
     * [MAX_KINDS] the oldest goes, never one in [kept]: a kind kept on the
     * phone must keep its row to be switched back on (O5 review). Null
     * when nothing changed, or [id] is blank.
     */
    fun seen(kinds: List<Kind>, id: String, example: String, kept: Set<String> = emptySet()): List<Kind>? {
        if (id.isBlank()) return null
        val known = kinds.indexOfFirst { it.id == id }
        if (known >= 0) {
            if (kinds[known].example.isNotBlank() || example.isBlank()) return null
            return kinds.toMutableList().apply { this[known] = Kind(id, example) }
        }
        val next = (listOf(Kind(id, example)) + kinds).toMutableList()
        while (next.size > MAX_KINDS) {
            val oldest = next.indexOfLast { it.id !in kept }
            if (oldest < 0) break
            next.removeAt(oldest)
        }
        return next
    }

    /** The rows to show: [kinds], then any of [kept] they lack, untitled. */
    fun listed(kinds: List<Kind>, kept: Set<String>): List<Kind> =
        kinds + kept.filter { id -> kinds.none { it.id == id } }.sorted().map { Kind(it, "") }
}
