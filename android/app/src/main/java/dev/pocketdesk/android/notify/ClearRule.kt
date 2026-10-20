package dev.pocketdesk.android.notify

/**
 * Whether a clear from the computer clears the phone's notification (L1):
 * with "Let the computer clear my notifications" on, from the phone's own
 * computer, for a notification the phone sent it, still shown and
 * clearable, never a call (F5), within [PER_MINUTE]. Pure, for JVM tests.
 */
object ClearRule {
    /** Clears taken in any minute, at most. */
    const val PER_MINUTE = 30

    enum class Decision { Clear, Off, NotOwnComputer, NotSent, Gone, NotClearable, Call, TooMany }

    fun decide(
        settingOn: Boolean,
        ownComputer: Boolean,
        sent: Boolean,
        shown: Boolean,
        clearable: Boolean,
        call: Boolean,
        withinBudget: Boolean,
    ): Decision =
        when {
            !settingOn -> Decision.Off
            !ownComputer -> Decision.NotOwnComputer
            call -> Decision.Call
            !sent -> Decision.NotSent
            !shown -> Decision.Gone
            !clearable -> Decision.NotClearable
            !withinBudget -> Decision.TooMany
            else -> Decision.Clear
        }
}
