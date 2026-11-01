package dev.pocketdesk.watchlink

/**
 * The things a watch may ask this phone to do, each its own switch in the
 * phone's Settings ("Watch controls"). A button only ever asks the phone to
 * do what the phone's own widgets and tiles already do for its owner, so the
 * computer sees the phone and nothing else: every button stays held to the
 * permission the computer already gave this phone.
 */
enum class WatchButton(val bit: Int, val key: String) {
    /** Wake the computer the phone last worked with. */
    Wake(1, "wake"),

    /** Ring the phone itself (the alarm sound, 30 s, Stop on the phone). */
    Ring(2, "ring"),

    /** The computer's music: play or pause, next, previous. */
    Music(4, "music"),

    /** The computer's deck buttons. */
    Deck(8, "deck"),

    /** See what the computer is asking and say No to it. */
    Asks(16, "asks"),
    ;

    companion object {
        /**
         * On once Watch controls is on: the harmless and the stopping ones.
         * A deck button may run a task on the computer, so it waits for a
         * choice.
         */
        val DEFAULT: Set<WatchButton> = setOf(Wake, Ring, Music, Asks)

        fun mask(buttons: Set<WatchButton>): Int = buttons.fold(0) { acc, b -> acc or b.bit }

        fun fromMask(mask: Int): Set<WatchButton> = entries.filter { mask and it.bit != 0 }.toSet()
    }
}

/** What a frame from the watch asks for. */
enum class WatchOp(val code: Int, val button: WatchButton?) {
    /** The computer's name, the buttons that are on, how many asks wait. */
    Status(1, null),
    Wake(2, WatchButton.Wake),
    Ring(3, WatchButton.Ring),

    /** Body: one byte, a [MusicCommand]. */
    Music(4, WatchButton.Music),
    DeckList(5, WatchButton.Deck),

    /** Body: the button's index (u16) and its label as listed. */
    DeckPress(6, WatchButton.Deck),
    AskList(7, WatchButton.Asks),

    /** Body: the ask's id. Only No: a Yes needs the phone's own fingerprint. */
    AskDeny(8, WatchButton.Asks),
    ;

    companion object {
        fun of(code: Int): WatchOp? = entries.firstOrNull { it.code == code }
    }
}

/** The music commands, as the computer's `mediaControl` names them. */
enum class MusicCommand(val code: Int, val command: String) {
    PlayPause(0, "playPause"),
    Next(1, "next"),
    Previous(2, "previous"),
    ;

    companion object {
        fun of(code: Int): MusicCommand? = entries.firstOrNull { it.code == code }
    }
}

/** How the phone answered. */
enum class WatchStatus(val code: Int) {
    Ok(0),

    /** The switch for that button is off, or the watch is not allowed. */
    Refused(1),

    /** The phone tried and it did not work; the body is why, in a few words. */
    Failed(2),

    /** Too soon after the last one. */
    Busy(3),
    ;

    companion object {
        fun of(code: Int): WatchStatus? = entries.firstOrNull { it.code == code }
    }
}
