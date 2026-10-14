package dev.pocketdesk.android.notify

/** A reply the desktop asked this phone to text (N5b `kdeconnect.sms.request`). */
data class SmsRequest(val id: String, val address: String, val body: String)

/**
 * Whether a reply from the computer is sent (N5b), and how incoming texts are
 * put together. Pure, for JVM tests.
 */
object SmsReplyRule {
    const val OFF = "replies from the computer are off on the phone"
    const val NO_PERMISSION = "the phone does not let Pocketdesk send texts"
    const val BAD_NUMBER = "that is not a phone number"
    const val BAD_TEXT = "the text is empty or too long"

    /** The longest reply sent, in characters (split into parts by Android). */
    const val MAX_TEXT = 2000

    private val NUMBER = Regex("""^\+?[0-9][0-9 ()\-.]{1,30}$""")

    sealed interface Decision {
        data object Send : Decision

        data class Refuse(val reason: String) : Decision
    }

    fun decide(request: SmsRequest, replyOn: Boolean, permitted: Boolean): Decision {
        if (!replyOn) return Decision.Refuse(OFF)
        if (!permitted) return Decision.Refuse(NO_PERMISSION)
        val digits = request.address.count { it.isDigit() }
        if (!NUMBER.matches(request.address.trim()) || digits !in 2..20) return Decision.Refuse(BAD_NUMBER)
        val text = request.body.trim()
        if (text.isEmpty() || text.length > MAX_TEXT) return Decision.Refuse(BAD_TEXT)
        return Decision.Send
    }

    /** The parts of one incoming broadcast, joined per sender in order. */
    fun joinParts(parts: List<Pair<String, String>>): List<Pair<String, String>> {
        val joined = LinkedHashMap<String, StringBuilder>()
        for ((address, body) in parts) joined.getOrPut(address) { StringBuilder() }.append(body)
        return joined.map { (address, body) -> address to body.toString() }
    }
}
