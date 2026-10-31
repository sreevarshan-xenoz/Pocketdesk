package dev.pocketdesk.android.ui

/**
 * A computer card's menu, in headed sections (UX2): it was 13 items in one
 * flat list, longer than the screen and hard to scan. What it does there
 * first, then what the phone sends it, then its upkeep folded under one
 * "Manage" row that opens in place, Remove last. Every action stays in it.
 * Kept apart from Android so the grouping is tested.
 */
object CardMenu {
    enum class Action {
        WAKE, PEEK, SOUND, DECK, STATE,
        CAMERA, PHONE_SOUND, EARLIER_COPY,
        CHECK, WAKE_SETTINGS, ALLOWED, DEVICES, INTRODUCE, RENAME, REMOVE,
    }

    enum class Kind {
        /** What it does on the computer: always open. */
        COMPUTER,

        /** What this phone sends to it: always open. */
        PHONE,

        /** Its upkeep: behind "Manage" until that is tapped. */
        MANAGE,
    }

    data class Section(val kind: Kind, val actions: List<Action>) {
        val folded: Boolean get() = kind == Kind.MANAGE
    }

    fun sections(peek: Boolean, phoneSound: Boolean, hasWake: Boolean): List<Section> = listOf(
        Section(
            Kind.COMPUTER,
            buildList {
                // An asleep computer's first need.
                if (hasWake) add(Action.WAKE)
                // BF2: a glance at its screen: the quickest.
                if (peek) add(Action.PEEK)
                add(Action.SOUND)
                add(Action.DECK)
                add(Action.STATE)
            },
        ),
        Section(
            Kind.PHONE,
            buildList {
                add(Action.CAMERA)
                // M7: Android 10 and later only.
                if (phoneSound) add(Action.PHONE_SOUND)
                add(Action.EARLIER_COPY)
            },
        ),
        Section(
            Kind.MANAGE,
            listOf(
                Action.CHECK,
                Action.WAKE_SETTINGS,
                Action.ALLOWED,
                Action.DEVICES,
                // Ecosystem phase 4: the owner's other computers, added here too.
                Action.INTRODUCE,
                Action.RENAME,
                Action.REMOVE,
            ),
        ),
    )
}
