package dev.pocketdesk.android.ui

/** When the pairing sheet takes a PIN, kept apart from Compose so it is tested. */
object PinRules {
    /**
     * Six digits, and not already paired. Waiting for the desktop after
     * "Show PIN on desktop" does not block it: that PIN is the one to enter.
     */
    fun canSubmit(pin: String, success: Boolean): Boolean = pin.length == 6 && !success
}
