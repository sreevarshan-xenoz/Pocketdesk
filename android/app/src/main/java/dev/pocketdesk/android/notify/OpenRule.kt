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
        LOCKED,
        GONE,
        TOO_MANY,
    }

    /**
     * Only from the phone's own computer with the switch on; never while
     * the phone is locked; only a notification still shown that has a tap
     * action ([shown]), within the answers' budget.
     */
    fun decide(
        settingOn: Boolean,
        ownComputer: Boolean,
        locked: Boolean,
        shown: Boolean,
        service: Boolean,
        withinBudget: Boolean,
    ): Decision = when {
        !ownComputer -> Decision.NOT_OWN_COMPUTER
        !settingOn -> Decision.OFF
        !shown -> Decision.GONE
        locked -> Decision.LOCKED
        !withinBudget -> Decision.TOO_MANY
        service -> Decision.OPEN
        else -> Decision.OFFER
    }

    /**
     * What the computer hears (E1): [done] is whether the open or the offer
     * went through; nothing for another's computer, which is never told.
     */
    fun outcome(decision: Decision, done: Boolean): NotificationActionRule.Outcome? = when (decision) {
        Decision.NOT_OWN_COMPUTER -> null
        Decision.OPEN -> NotificationActionRule.Outcome(done, if (done) null else "the app did not open")
        Decision.OFFER -> NotificationActionRule.Outcome(done, if (done) null else "the phone could not show it")
        Decision.OFF -> NotificationActionRule.Outcome(false, "opening is off on the phone")
        Decision.LOCKED -> NotificationActionRule.Outcome(false, "the phone is locked")
        Decision.GONE -> NotificationActionRule.Outcome(false, "it is gone on the phone")
        Decision.TOO_MANY -> NotificationActionRule.Outcome(false, "too many in a minute")
    }
}
