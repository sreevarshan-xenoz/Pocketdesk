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

    /** The start of each still's file name in the app's cache. */
    const val FILE_PREFIX = "peek"

    /** A computer sends at most 1600 on the longest side; a little more is let through. */
    private const val LONGEST_SIDE = 2048

    /** Whether a still of [width] x [height] is one a computer sends. */
    fun fits(width: Int, height: Int): Boolean =
        width in 1..LONGEST_SIDE && height in 1..LONGEST_SIDE

    /** The stills an app stopped mid-peek left in [files]. */
    fun leftovers(files: List<java.io.File>): List<java.io.File> =
        files.filter { it.isFile && it.name.startsWith(FILE_PREFIX) && it.name.endsWith(".jpg") }
}
