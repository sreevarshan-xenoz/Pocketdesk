package dev.pocketdesk.android.ui

/**
 * Why a peek (BF2) brought no still, from the error the bridge returns: the
 * computer's own reason when it refused, else [Other] with the error.
 * Kept apart from Android so it is tested.
 */
object Peek {
    enum class Refusal { Off, Paused, Busy, TooSoon, NoScreen, NotAllowed, Other }

    private val CODES = mapOf(
        "off" to Refusal.Off,
        "paused" to Refusal.Paused,
        "busy" to Refusal.Busy,
        "too_soon" to Refusal.TooSoon,
        "no_screen" to Refusal.NoScreen,
        "not_allowed" to Refusal.NotAllowed,
    )

    fun refusal(message: String?): Refusal = CODES[message?.trim()] ?: Refusal.Other
}
