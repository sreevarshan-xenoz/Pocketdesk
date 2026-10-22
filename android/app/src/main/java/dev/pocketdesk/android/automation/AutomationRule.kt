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
        /** "Let other apps run the computer's tasks" is off. */
        OFF("not_allowed"),

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
     * Why the ask is refused, or null to send it: the switch [on], a
     * [saved] computer, a [name] that passed [name], and fewer than
     * [PER_MINUTE] of the asks at [recentMs] within a minute of [nowMs].
     */
    fun refusal(on: Boolean, saved: Boolean, name: String?, recentMs: List<Long>, nowMs: Long): Refusal? =
        when {
            !on -> Refusal.OFF
            !saved -> Refusal.NOT_SAVED
            name == null -> Refusal.BAD_NAME
            recentMs.count { nowMs - it in 0 until MINUTE_MS } >= PER_MINUTE -> Refusal.TOO_MANY
            else -> null
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
