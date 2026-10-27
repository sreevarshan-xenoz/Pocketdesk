package dev.pocketdesk.android.ask

/**
 * The rules for a question the computer puts to this phone (BF3), kept apart
 * from Android so they are tested. No is one tap: it can only stop
 * something. Yes takes the phone's fingerprint or screen lock, so a phone
 * with no lock cannot say Yes.
 */
object AskRule {
    /** The answer's packet. */
    const val ANSWER = "kdeconnect.pocketdesk.ask_answer"

    enum class Decision { Offer, Off, NotOwnComputer, Expired }

    private val ID = Regex("^[A-Za-z0-9-]{1,64}$")

    fun decide(on: Boolean, ownComputer: Boolean, expiresUnix: Long, nowUnix: Long): Decision = when {
        !on -> Decision.Off
        !ownComputer -> Decision.NotOwnComputer
        nowUnix >= expiresUnix -> Decision.Expired
        else -> Decision.Offer
    }

    /** Whether Yes may be said: only behind a lock the phone has. */
    fun mayAnswerYes(deviceSecure: Boolean): Boolean = deviceSecure

    /** The answer's body, or null for an id the computer would not take. */
    fun body(id: String, yes: Boolean): String? =
        if (ID.matches(id)) """{"id":"$id","answer":"${if (yes) "yes" else "no"}"}""" else null
}
