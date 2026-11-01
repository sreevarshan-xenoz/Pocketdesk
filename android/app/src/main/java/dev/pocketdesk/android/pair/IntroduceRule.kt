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

    /**
     * The ids of the computers this phone is still paired with, from its
     * cards (address to the id it paired with, if any): a card whose
     * computer said it no longer pairs with this phone (BB3, "No longer
     * paired") counts as not paired, so that computer can be offered again.
     */
    fun pairedIds(cards: Map<String, String?>, unpairedBy: (String) -> Boolean): Set<String> =
        cards.filter { (address, id) -> id != null && !unpairedBy(address) }.values.filterNotNull().toSet()

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

    /**
     * The pairing link an answer hands over, or null: only an answer to this
     * ask ([asked]), only a pairing link, and only one to the computer that
     * was asked for ([computer]).
     */
    fun takeLink(asked: String, answered: String?, computer: String, link: String?): PairLink? {
        if (!isAnswerTo(asked, answered) || !isPairLink(link)) return null
        return PairLink.parse(link!!).getOrNull()?.takeIf { linkFits(it, computer) }
    }

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

    /** How long a computer's own offer stays quiet after Not now (in ms). */
    const val NOT_NOW_QUIET_MS = 30 * 60_000L

    /**
     * One "Also pair with" sheet at a time, and Not now heard: once the
     * owner passes a computer's offer made by itself after pairing, that
     * computer does not offer again by itself for [quietMs] on this run
     * of the app (a second sheet followed Not now once on the OPPO). The
     * card's menu still asks whenever it is tapped, but never over a sheet
     * already open. Kept in memory: a new run of the app may offer again.
     */
    class Gate(private val quietMs: Long = NOT_NOW_QUIET_MS) {
        private var open: String? = null
        private val passed = mutableMapOf<String, Long>()

        /** Whether the computer at [address] may offer by itself at [now]. */
        @Synchronized
        fun mayOfferByItself(address: String, now: Long): Boolean {
            if (open != null) return false
            val at = passed[address] ?: return true
            return now - at >= quietMs || now < at
        }

        /** How long [address]'s own offer stays quiet from [now] (0: not quiet). */
        @Synchronized
        fun quietLeftMs(address: String, now: Long): Long {
            val at = passed[address] ?: return 0
            if (now < at) return 0
            return (at + quietMs - now).coerceAtLeast(0)
        }

        /** Whether a sheet for [address] may open now; if so, it is the open one. */
        @Synchronized
        fun open(address: String): Boolean {
            if (open != null && open != address) return false
            open = address
            return true
        }

        /** The sheet for [address] closed. */
        @Synchronized
        fun closed(address: String) {
            if (open == address) open = null
        }

        /** The owner said Not now to [address]'s offer (or closed it unanswered) at [now]. */
        @Synchronized
        fun notNow(address: String, now: Long) {
            passed[address] = now
        }
    }

    /** What to do with an offer waiting to be made by itself after pairing. */
    sealed interface Pending {
        /** The owner turned these offers off: forget it. */
        data object Drop : Pending

        /** A sheet is open: keep it until that one closes. */
        data object Hold : Pending

        /** Quiet after Not now: keep it, and look again in [ms]. */
        data class After(val ms: Long) : Pending

        /** Show it now. */
        data object Now : Pending
    }

    fun pendingStep(offerOn: Boolean, sheetOpen: Boolean, quietLeftMs: Long): Pending = when {
        !offerOn -> Pending.Drop
        sheetOpen -> Pending.Hold
        quietLeftMs > 0 -> Pending.After(quietLeftMs)
        else -> Pending.Now
    }

    /** This run of the app's [Gate]. */
    val gate = Gate()
}
