package dev.pocketdesk.android.notify

/**
 * A reply the desktop asked this phone to text (N5b `kdeconnect.sms.request`),
 * with the computer whose connection carried it.
 */
data class SmsRequest(
    val id: String,
    val address: String,
    val body: String,
    val fromHost: String,
    val fromPort: Int,
)

/**
 * Whether a reply from the computer is sent (N5b), and how incoming texts are
 * put together. Pure, for JVM tests.
 */
object SmsReplyRule {
    const val OFF = "replies from the computer are off on the phone"
    const val NO_PERMISSION = "the phone does not let Pocketdesk send texts"
    const val NOT_YOURS = "this phone takes replies only from its own computer"
    const val BAD_NUMBER = "that is not a full phone number"
    const val BAD_TEXT = "the text is empty or too long"
    const val TOO_MANY = "the phone sends at most 5 replies a minute"

    /** The longest reply sent, in characters (split into parts by Android). */
    const val MAX_TEXT = 2000

    private val NUMBER = Regex("""^\+?[0-9][0-9 ()\-.]{1,30}$""")

    sealed interface Decision {
        data object Send : Decision

        data class Refuse(val reason: String) : Decision
    }

    /**
     * Send only when the user turned replies on, Android allows it, the
     * request came from the phone's own computer, and it is a full number
     * (7 to 20 digits: never a premium short code) with some text.
     */
    fun decide(request: SmsRequest, replyOn: Boolean, permitted: Boolean, fromOwnComputer: Boolean): Decision {
        if (!replyOn) return Decision.Refuse(OFF)
        if (!permitted) return Decision.Refuse(NO_PERMISSION)
        if (!fromOwnComputer) return Decision.Refuse(NOT_YOURS)
        val digits = request.address.count { it.isDigit() }
        if (!NUMBER.matches(request.address.trim()) || digits !in 7..20) return Decision.Refuse(BAD_NUMBER)
        val text = request.body.trim()
        if (text.isEmpty() || text.length > MAX_TEXT) return Decision.Refuse(BAD_TEXT)
        return Decision.Send
    }

    /** At most [MAX] replies a minute, whatever asks for them. */
    class SendBudget {
        private val sent = ArrayDeque<Long>()

        fun take(nowMs: Long): Boolean {
            while (sent.isNotEmpty() && nowMs - sent.first() >= WINDOW_MS) sent.removeFirst()
            if (sent.size >= MAX) return false
            sent.addLast(nowMs)
            return true
        }

        companion object {
            const val MAX = 5
            const val WINDOW_MS = 60_000L
        }
    }

    /** The parts of one incoming broadcast, joined per sender in order. */
    fun joinParts(parts: List<Pair<String, String>>): List<Pair<String, String>> {
        val joined = LinkedHashMap<String, StringBuilder>()
        for ((address, body) in parts) joined.getOrPut(address) { StringBuilder() }.append(body)
        return joined.map { (address, body) -> address to body.toString() }
    }
}
