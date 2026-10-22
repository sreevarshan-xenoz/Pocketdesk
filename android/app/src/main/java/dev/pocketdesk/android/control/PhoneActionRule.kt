package dev.pocketdesk.android.control

/**
 * O2: whether this phone does what its computer asks of it, a screenshot
 * or a lock, through C8's control service. Pure, for JVM tests;
 * [PhoneActions] does it.
 */
object PhoneActionRule {
    /** How many asks a minute are done. */
    const val PER_MINUTE = 10

    /** The Android version the service's screenshot and lock need (P). */
    const val MIN_SDK = 28

    enum class Action(val word: String) {
        SCREENSHOT("screenshot"),
        LOCK("lock"),
        ;

        companion object {
            fun parse(word: String?): Action? = entries.firstOrNull { it.word == word }
        }
    }

    enum class Decision {
        DO,

        /** "Let the computer take screenshots and lock this phone" is off. */
        OFF,

        /** Not this phone's own computer. */
        NOT_OWN,

        /** The Pocketdesk control service is not on. */
        NO_SERVICE,

        /** Android before 9 has neither action. */
        TOO_OLD,

        /** A screenshot of the lock screen is never taken. */
        LOCKED,

        /**
         * A screenshot with the screen off would be of nothing: Android
         * takes none, though the service says it did (found on the OPPO).
         */
        ASLEEP,

        /** A screenshot would not reach the computer: "Send new screenshots" is off. */
        SCREENSHOTS_OFF,

        /** More than [PER_MINUTE] in a minute. */
        TOO_MANY,
    }

    /**
     * What to do with [action] from a computer: [on] the phone's switch,
     * [own] whether it is the phone's own computer, [service] whether the
     * control service runs, [sdk] Android's version, [locked] whether the
     * phone is locked, [awake] whether its screen is on, [screenshots]
     * whether T10h sends new screenshots, and
     * [withinBudget] whether the minute allows another.
     */
    fun decide(
        action: Action,
        on: Boolean,
        own: Boolean,
        service: Boolean,
        sdk: Int,
        locked: Boolean,
        awake: Boolean,
        screenshots: Boolean,
        withinBudget: Boolean,
    ): Decision = when {
        !on -> Decision.OFF
        !own -> Decision.NOT_OWN
        sdk < MIN_SDK -> Decision.TOO_OLD
        !service -> Decision.NO_SERVICE
        action == Action.SCREENSHOT && locked -> Decision.LOCKED
        action == Action.SCREENSHOT && !awake -> Decision.ASLEEP
        action == Action.SCREENSHOT && !screenshots -> Decision.SCREENSHOTS_OFF
        !withinBudget -> Decision.TOO_MANY
        else -> Decision.DO
    }
}
