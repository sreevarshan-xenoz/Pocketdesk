package dev.pocketdesk.android.control

/**
 * O2: whether this phone does what its computer asks of it, a screenshot
 * or a lock, through C8's control service; P2: its apps listed, one
 * opened. Pure, for JVM tests; [PhoneActions] does it.
 */
object PhoneActionRule {
    /** How many asks a minute are done. */
    const val PER_MINUTE = 10

    /** The Android version the service's screenshot and lock need (P). */
    const val MIN_SDK = 28

    /** The most apps listed for the computer (P2), as it keeps at most. */
    const val MAX_APPS = 500

    enum class Action(val word: String) {
        SCREENSHOT("screenshot"),
        LOCK("lock"),

        /** P2: list the apps that open. */
        APPS("apps"),

        /** P2: open one. */
        OPEN("open"),

        /** P10: list the contacts, names and numbers only. */
        CONTACTS("contacts"),
        ;

        companion object {
            fun parse(word: String?): Action? = entries.firstOrNull { it.word == word }
        }
    }

    enum class Decision {
        DO,

        /**
         * "Let the computer take screenshots and lock this phone" is off,
         * or for [Action.APPS] and [Action.OPEN], "Let the computer open my
         * apps".
         */
        OFF,

        /** Not this phone's own computer. */
        NOT_OWN,

        /** The Pocketdesk control service is not on. */
        NO_SERVICE,

        /** Android before 9 has neither action. */
        TOO_OLD,

        /** A screenshot of the lock screen is never taken, nor an app opened behind it. */
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

        /** P2: an app the phone did not list for the computer. */
        NOT_LISTED,

        /** P10: Android has not let this app read the contacts. */
        NO_PERMISSION,
    }

    /**
     * What to do with [action] from a computer: [on] the phone's switch,
     * [own] whether it is the phone's own computer, [service] whether the
     * control service runs, [sdk] Android's version, [locked] whether the
     * phone is locked, [awake] whether its screen is on, [screenshots]
     * whether T10h sends new screenshots, [appsOn] whether the computer
     * may list and open apps (P2), and [withinBudget] whether the minute
     * allows another.
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
        appsOn: Boolean = false,
        listed: Boolean = false,
        contactsOn: Boolean = false,
        contactsPermitted: Boolean = false,
    ): Decision = when {
        // P10: the contacts, with their own switch and Android's leave; no
        // service, and shown on nothing.
        action == Action.CONTACTS -> when {
            !contactsOn -> Decision.OFF
            !own -> Decision.NOT_OWN
            !contactsPermitted -> Decision.NO_PERMISSION
            !withinBudget -> Decision.TOO_MANY
            else -> Decision.DO
        }
        // P2: a list needs no service and shows nothing on the phone; an
        // open needs the service (Android lets a bound one start an app
        // from the background), an unlocked phone, awake.
        // The list, only for an unlocked phone (P2 review: it names every app).
        action == Action.APPS -> when {
            !appsOn -> Decision.OFF
            !own -> Decision.NOT_OWN
            locked -> Decision.LOCKED
            !withinBudget -> Decision.TOO_MANY
            else -> Decision.DO
        }
        // An open needs no service: without it, "Tap to open" (P2 review).
        action == Action.OPEN -> when {
            !appsOn -> Decision.OFF
            !own -> Decision.NOT_OWN
            !listed -> Decision.NOT_LISTED
            locked -> Decision.LOCKED
            !awake -> Decision.ASLEEP
            !withinBudget -> Decision.TOO_MANY
            else -> Decision.DO
        }
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
