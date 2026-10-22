package dev.pocketdesk.android.notify

/**
 * Toasts only on the computer in use (N7): with "Toast only on the computer
 * I'm using" on, a notification to a computer that says it is not in use,
 * while another computer of the owner's says it is, is marked so that one
 * keeps it without a pop-up. A computer that has not answered lately, or
 * does not say, is never marked: with no computer known to be in use,
 * every one toasts. Kept apart from Android so it is tested.
 */
object ToastWhere {
    /** An answer older than this is not counted (a link that dropped). */
    const val FRESH_MS = 30_000L

    /** How often each linked computer is asked. */
    const val ASK_EVERY_MS = 15_000L

    /** The field a marked notification carries (`core::protocol::in_use::ELSEWHERE`). */
    const val ELSEWHERE = "elsewhere"

    /** What a computer last said: in use or not (null: it does not say), and how long ago. */
    data class Answer(val inUse: Boolean?, val ageMs: Long)

    /** A computer's key in the answers: its address and control port. */
    fun key(address: String, port: Int): String = "$address:$port"

    /**
     * Whether a notification to [target] is marked [ELSEWHERE]: [on], and
     * [target] said lately it is not in use while another said lately it is.
     */
    fun marks(target: String, answers: Map<String, Answer>, on: Boolean): Boolean {
        if (!on) return false
        val mine = answers[target]?.takeIf { it.ageMs <= FRESH_MS } ?: return false
        if (mine.inUse != false) return false
        return answers.any { (key, answer) -> key != target && answer.ageMs <= FRESH_MS && answer.inUse == true }
    }

    /** Whether it is time to ask again, [sinceMs] after the last ask. */
    fun askDue(on: Boolean, sinceMs: Long): Boolean = on && sinceMs >= ASK_EVERY_MS
}
