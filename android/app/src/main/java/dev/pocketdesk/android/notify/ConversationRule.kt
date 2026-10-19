package dev.pocketdesk.android.notify

/**
 * Which of a chat notification's recent messages go to the computer (D4):
 * the newest [MAX], oldest first, none blank, and none that looks like a
 * one-time code (the same test as a notification's own text), which goes
 * as [hidden] instead. Kept apart from Android so it is tested.
 */
object ConversationRule {
    /** As the computer keeps at most. */
    const val MAX = 10

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

    fun messages(raw: List<Raw>, hidden: String): List<Out> =
        raw.mapNotNull { m ->
            val text = m.text?.trim().orEmpty()
            if (text.isEmpty()) return@mapNotNull null
            Out(
                from = if (m.mine) "" else m.from?.trim().orEmpty(),
                text = if (NotificationForwardRule.looksLikeCode(text)) hidden else text,
                timeMs = m.timeMs,
                mine = m.mine,
            )
        }.takeLast(MAX)
}
