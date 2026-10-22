package dev.pocketdesk.android.automation

import dev.pocketdesk.android.ui.Deck

/**
 * O1: what an automation app may ask of the computer through Pocketdesk,
 * a task started or a deck button pressed by its name. Any app on the
 * phone can send the intent, so it is taken only with the phone's
 * Settings "Let other apps run the computer's tasks" on, for a saved
 * computer, a short printable name, and at most [PER_MINUTE] a minute.
 * Pure, for JVM tests; [AutomationReceiver] does the asking.
 */
object AutomationRule {
    /** The longest task name or button label taken. */
    const val MAX_NAME = 64

    /** How many asks a minute, from all apps together. */
    const val PER_MINUTE = 10

    private const val MINUTE_MS = 60_000L

    /** Why an ask is not sent, as the event's `REASON` says it. */
    enum class Refusal(val reason: String) {
        /** The address names no saved computer. */
        NOT_SAVED("not_saved"),

        /** No name, or one too long or with control characters. */
        BAD_NAME("bad_name"),

        /** More than [PER_MINUTE] in the last minute. */
        TOO_MANY("too_many"),
    }

    /** [raw] trimmed, when it can name a task or a button; else null. */
    fun name(raw: String?): String? =
        raw?.trim()?.takeIf { it.isNotEmpty() && it.length <= MAX_NAME && it.none(Char::isISOControl) }

    /**
     * Why the ask is refused, or null to send it: a [saved] computer, a
     * [name] that passed [name], and fewer than [PER_MINUTE] of the asks at
     * [recentMs] within a minute of [nowMs]. The switch is read before any
     * of it (O1 review), so an ask with it off is never here.
     */
    fun refusal(saved: Boolean, name: String?, recentMs: List<Long>, nowMs: Long): Refusal? =
        when {
            !saved -> Refusal.NOT_SAVED
            name == null -> Refusal.BAD_NAME
            recentMs.count { nowMs - it in 0 until MINUTE_MS } >= PER_MINUTE -> Refusal.TOO_MANY
            else -> null
        }

    /** What a task's start does about the phone's one control link; [reason] as the event's. */
    enum class Link(val reason: String) {
        /** Linked to that computer already. */
        LINKED(""),

        /** No link, and that computer is the phone's own: brought up. */
        CONNECT(""),

        /** Linked to another computer, which keeps its link. */
        LINKED_ELSEWHERE("linked_elsewhere"),

        /** No link, and that computer is not the phone's own: a link to it would take the background features there. */
        NOT_LINKED("not_linked"),
    }

    /**
     * Whether a start for the computer [wanted] (address and port) may use
     * the link [linked] to, bring one up, or not, with [own] the phone's own
     * computer (null when it has none, so any saved one may be linked).
     */
    fun link(linked: Pair<String, Int>?, wanted: Pair<String, Int>, own: Pair<String, Int>?): Link =
        when {
            linked == wanted -> Link.LINKED
            linked != null -> Link.LINKED_ELSEWHERE
            own == null || own == wanted -> Link.CONNECT
            else -> Link.NOT_LINKED
        }

    /** The asks of [recentMs] still within a minute of [nowMs], to keep. */
    fun kept(recentMs: List<Long>, nowMs: Long): List<Long> = recentMs.filter { nowMs - it in 0 until MINUTE_MS }

    /** The button the deck lists as [label], exactly; the first when two share it. */
    fun button(buttons: List<Deck.Button>, label: String): Deck.Button? = buttons.firstOrNull { it.label == label }

    /**
     * A press told to other apps: its `STATE` (`done`, `waiting` for the
     * owner, `refused` by the computer, `not_sent`) and the computer's reason.
     */
    fun pressed(outcome: Deck.Outcome?): Map<String, Any?> =
        when {
            outcome == null -> mapOf("state" to "not_sent")
            outcome.waiting -> mapOf("state" to "waiting")
            outcome.ok -> mapOf("state" to "done")
            else -> mapOf("state" to "refused", "reason" to (outcome.reason ?: "refused"))
        }
}
