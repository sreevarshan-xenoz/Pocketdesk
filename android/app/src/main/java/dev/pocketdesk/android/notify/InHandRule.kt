package dev.pocketdesk.android.notify

/**
 * No toast while the phone is in hand (M6): a notification that comes while
 * the phone's screen is on and unlocked is marked `phoneInUse`, and the
 * computer keeps it without a pop-up.
 */
object InHandRule {
    /** The body field the computer reads. */
    const val FIELD = "phoneInUse"

    /** Mark it? With the switch on, the screen on, and the phone unlocked. */
    fun marks(on: Boolean, interactive: Boolean, locked: Boolean): Boolean = on && interactive && !locked
}
