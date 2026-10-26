package dev.pocketdesk.android.notify

/**
 * Whether the phone looks again at a computer that said it is not paired
 * (BB2): dialed every few minutes, sending nothing, so a computer that lets
 * the phone in again is heard; never at the cost of a link it did not open.
 */
object RecheckPlan {
    /** How often the computer is looked at again. */
    const val EVERY_MS = 5 * 60_000L

    /** How long that look holds the link, waiting for the computer to let the phone in. */
    const val HOLD_MS = 30_000L

    /**
     * How long the phone's own computer must have had no link before a look
     * may take the single link (review: a link down a moment is the own
     * computer's to take back).
     */
    const val OWN_GRACE_MS = 120_000L

    enum class Act { NONE, DIAL, CLOSE }

    /**
     * What to do now. [peer] is the address the single control link goes
     * to, if any; [refuser] the computer that said the phone is not paired;
     * [opened] whether the look itself opened the link that is up; [lastMs]
     * when it last dialed; [ownDownMs] how long the phone's own computer has
     * been without a link, null when it has none.
     */
    fun act(peer: String?, refuser: String?, opened: Boolean, now: Long, lastMs: Long, ownDownMs: Long?): Act = when {
        refuser == null -> Act.NONE
        // The link is up: let go only one the look opened, once it has had its hold.
        peer != null -> if (peer == refuser && opened && now - lastMs >= HOLD_MS) Act.CLOSE else Act.NONE
        ownDownMs != null && ownDownMs < OWN_GRACE_MS -> Act.NONE
        now - lastMs >= EVERY_MS -> Act.DIAL
        else -> Act.NONE
    }
}
