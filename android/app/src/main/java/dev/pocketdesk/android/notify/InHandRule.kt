package dev.pocketdesk.android.notify

/**
 * No toast while the phone is in hand (M6): a notification that comes while
 * the phone's screen is on and unlocked is marked `phoneInUse`, and the
 * computer keeps it without a pop-up.
 */
object InHandRule {
    /** The body field the computer reads. */
    const val FIELD = "phoneInUse"

    /**
     * Whether Pocketdesk's Remote is in front (M6 review): the phone is on
     * and unlocked then, but the owner is looking at the computer, so its
     * toasts still show. Set while that screen is resumed.
     */
    @Volatile
    var remoteInFront = false

    /**
     * Mark it? With the switch on, the screen on, the phone unlocked, and
     * not Pocketdesk's Remote in front. A phone with no screen lock reads as
     * unlocked whenever its screen is on.
     */
    fun marks(on: Boolean, interactive: Boolean, locked: Boolean, remote: Boolean): Boolean =
        on && interactive && !locked && !remote
}
