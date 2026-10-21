package dev.pocketdesk.android.notify

/**
 * Whether a computer's open of one of this phone's notifications is taken
 * (N3), and what to tell it. Kept apart from Android so it is tested.
 */
object OpenRule {
    enum class Decision {
        /** Send the notification's tap action: the Accessibility service lets it start the app. */
        OPEN,

        /** Post "Tap to open" with that tap action: no service to start it with. */
        OFFER,
        OFF,
        NOT_OWN_COMPUTER,
        /** Not one sent to the computer offering to be opened (N3 review). */
        NOT_OFFERED,
        LOCKED,
        GONE,
        TOO_MANY,
    }

    /**
     * Only from the phone's own computer with the switch on; only one sent
     * to the computer offering to be opened ([offered]); never while the
     * phone is locked or asleep; only a notification still shown that has
     * a tap action ([shown]), within the answers' budget.
     */
    fun decide(
        settingOn: Boolean,
        ownComputer: Boolean,
        offered: Boolean,
        locked: Boolean,
        shown: Boolean,
        service: Boolean,
        withinBudget: Boolean,
    ): Decision = when {
        !ownComputer -> Decision.NOT_OWN_COMPUTER
        !settingOn -> Decision.OFF
        !offered -> Decision.NOT_OFFERED
        !shown -> Decision.GONE
        locked -> Decision.LOCKED
        !withinBudget -> Decision.TOO_MANY
        service -> Decision.OPEN
        else -> Decision.OFFER
    }

    /** What a computer hears of an open that went (N3 review: an offer is not an open). */
    const val OPENED = "opened on the phone"
    const val OFFERED = "offered on the phone: tap it there"

    /**
     * What the computer hears (E1): [done] is whether the open or the offer
     * went through, said on success too so the computer can tell them
     * apart; nothing for another's computer, which is never told.
     */
    fun outcome(decision: Decision, done: Boolean): NotificationActionRule.Outcome? = when (decision) {
        Decision.NOT_OWN_COMPUTER -> null
        Decision.OPEN -> NotificationActionRule.Outcome(done, if (done) OPENED else "the app did not open")
        Decision.OFFER -> NotificationActionRule.Outcome(done, if (done) OFFERED else "the phone could not show it")
        Decision.NOT_OFFERED -> NotificationActionRule.Outcome(false, "it was not offered to be opened")
        Decision.OFF -> NotificationActionRule.Outcome(false, "opening is off on the phone")
        Decision.LOCKED -> NotificationActionRule.Outcome(false, "the phone is locked")
        Decision.GONE -> NotificationActionRule.Outcome(false, "it is gone on the phone")
        Decision.TOO_MANY -> NotificationActionRule.Outcome(false, "too many in a minute")
    }
}
