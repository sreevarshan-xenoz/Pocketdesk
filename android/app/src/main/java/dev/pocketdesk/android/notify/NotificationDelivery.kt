package dev.pocketdesk.android.notify

/**
 * Whether a phone notification reached the computer (C3), from what the
 * bridge says became of it. A notification written into a control link that
 * had died unseen was counted as sent and lost, so the phone asks a computer
 * whose identity says it answers to say it read it:
 *
 * - `acked` or `refused`: the computer read it (and may have refused it);
 * - `unanswered`: nothing came back in time, so the link has died, and the
 *   notification waits;
 * - `pushed`: sent on a connection of its own, which cannot carry the answer;
 * - `sent`: not asked (a cancel, or a computer from before C3), as before.
 *
 * Kept apart from Android so it is tested.
 */
object NotificationDelivery {
    /** Whether [outcome] means the notification needs no sending again. */
    fun sent(outcome: String): Boolean = outcome in setOf("acked", "refused", "pushed", "sent")
}
