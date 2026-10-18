package dev.pocketdesk.android.notify

/**
 * Whether a phone notification reached the computer (C3), from what the
 * bridge says became of it. A notification written into a control link that
 * had died unseen was counted as sent and lost, so the phone asks the
 * computer to answer:
 *
 * - `acked` or `refused`: the computer read it (and may have refused it);
 * - `unanswered`: nothing came back in time. From a computer that has
 *   answered before, the link has died, and the notification waits; from
 *   one that never has (from before C3), it went as it always did;
 * - `pushed`: sent on a connection of its own, which cannot carry the answer.
 *
 * Kept apart from Android so it is tested.
 */
object NotificationDelivery {
    /** Whether [outcome] means the notification needs no sending again. */
    fun sent(outcome: String, computerAnswers: Boolean): Boolean =
        when (outcome) {
            "acked", "refused", "pushed" -> true
            "unanswered" -> !computerAnswers
            else -> false
        }

    /** Whether [outcome] shows the computer answers notifications. */
    fun answered(outcome: String): Boolean = outcome == "acked" || outcome == "refused"
}
