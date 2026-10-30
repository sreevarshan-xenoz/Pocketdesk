package dev.pocketdesk.android.pair

import dev.pocketdesk.android.R

/**
 * One pairing for all the owner's devices (ecosystem phase 4): a computer
 * this phone is paired with by its own code offers the owner's other
 * computers it is paired with both ways, and introduces the phone to the
 * one its owner says yes to here. Nothing is added without that yes. Kept
 * free of Android and org.json so the JVM tests reach it.
 */
object IntroduceRule {
    /** A computer this phone could be introduced to. */
    data class Offer(val id: String, val name: String)

    /** How the last introduction went, as the phone's Settings shows it. */
    enum class Kind { PAIRED, REFUSED, UNREACHABLE, EXPIRED }

    data class Outcome(val kind: Kind, val name: String, val code: String = "")

    /** A computer as shown: its name, else the start of its id. */
    fun shownName(offer: Offer): String = offer.name.trim().ifEmpty { offer.id.take(8) }

    /** The offers to ask about: not one this phone is already paired with, each once. */
    fun fresh(offers: List<Offer>, pairedIds: Set<String>): List<Offer> =
        offers.filter { it.id.isNotBlank() && it.id !in pairedIds }.distinctBy { it.id }

    /** What the owner reads for a computer's refusal code. */
    fun refusal(code: String?): Int = when (code) {
        "not_owner" -> R.string.introduce_refused_not_owner
        "not_mine" -> R.string.introduce_refused_not_mine
        "not_linked" -> R.string.introduce_refused_not_linked
        "off" -> R.string.introduce_refused_off
        "taken" -> R.string.introduce_refused_taken
        "no_key" -> R.string.introduce_refused_no_key
        "busy" -> R.string.introduce_refused_busy
        "no_answer" -> R.string.introduce_refused_no_answer
        else -> R.string.introduce_refused_failed
    }

    /** Whether [link] is a pairing link this app takes: the computer's answer is its own word. */
    fun isPairLink(link: String?): Boolean = link != null && link.startsWith("pocketdesk://pair?")

    /** Whether a pairing link names the computer that was asked for. */
    fun linkFits(link: PairLink, asked: String): Boolean = link.deviceId == asked

    /** A fresh id for one ask: the computer's answer names it. */
    fun newRequest(): String = java.util.UUID.randomUUID().toString().replace("-", "")

    /** Whether an answer naming [answered] is the one to the ask [asked]. */
    fun isAnswerTo(asked: String, answered: String?): Boolean = asked.isNotEmpty() && answered == asked

    /** [outcome] as kept in the phone's preferences. */
    fun encode(outcome: Outcome): String =
        listOf(outcome.kind.name, outcome.code, outcome.name.replace('\n', ' ')).joinToString("\n")

    /** An outcome kept by [encode]; null for none or one that does not read. */
    fun decode(kept: String?): Outcome? {
        val parts = kept?.split('\n', limit = 3) ?: return null
        if (parts.size != 3) return null
        val kind = Kind.entries.firstOrNull { it.name == parts[0] } ?: return null
        return Outcome(kind, parts[2], parts[1])
    }
}
