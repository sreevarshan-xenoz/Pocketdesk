package dev.pocketdesk.android.notify

/**
 * Which of a chat notification's recent messages go to the computer (D4):
 * the newest [MAX], oldest first, none blank, each cut to the computer's
 * limits. None at all when any of them, with its sender, looks like a
 * one-time code (the test a notification's own title and text get), or two
 * in a row from one sender do together (D4 review: a code split over two
 * messages, or named in the sender). Kept apart from Android so it is tested.
 */
object ConversationRule {
    /** As the computer keeps at most. */
    const val MAX = 10

    /** A message's text and a sender's name, at most, as the computer cuts them. */
    const val MAX_TEXT = 1000
    const val MAX_FROM = 64

    /** One message as the messaging app gave it. */
    data class Raw(val from: String?, val text: String?, val timeMs: Long, val mine: Boolean)

    /** One message as it is sent. */
    data class Out(val from: String, val text: String, val timeMs: Long, val mine: Boolean)

    /**
     * What tells one thread from another in what was last sent (D4 review):
     * a new message under the same title and text is something new to send.
     * Empty for no thread.
     */
    fun digest(thread: String?): String = if (thread == null) "" else "|m" + thread.hashCode()

    fun messages(raw: List<Raw>): List<Out> {
        val kept = raw.mapNotNull { m ->
            val text = m.text?.trim().orEmpty()
            if (text.isEmpty()) return@mapNotNull null
            Out(
                from = if (m.mine) "" else m.from?.trim().orEmpty().take(MAX_FROM),
                text = text.take(MAX_TEXT),
                timeMs = m.timeMs,
                mine = m.mine,
            )
        }.takeLast(MAX)
        val code = kept.any { NotificationForwardRule.looksLikeCode("${it.from} ${it.text}") } ||
            kept.zipWithNext().any { (a, b) ->
                a.from == b.from && a.mine == b.mine && NotificationForwardRule.looksLikeCode("${a.text} ${b.text}")
            }
        return if (code) emptyList() else kept
    }
}
