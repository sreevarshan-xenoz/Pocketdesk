package dev.pocketdesk.android.notify

/**
 * What pictures go with a notification to the computer (K3): each app's
 * icon once, and with "Pictures in notifications" on, the notification's
 * own picture, only when it is sent in full. Pure, for JVM tests.
 */
object NotificationPictures {
    /** The longest side of an app's icon, in pixels. */
    const val ICON_PX = 64

    /** The longest side of a notification's picture, in pixels. */
    const val PICTURE_PX = 720

    /** Whether [pkg]'s icon goes now: the first of its notifications this computer gets since the app started. */
    fun sendIcon(sent: Set<String>, pkg: String): Boolean = pkg !in sent

    /** Whether the notification's picture goes: the setting on, and the notification sent in full (never redacted). */
    fun sendPicture(enabled: Boolean, full: Boolean): Boolean = enabled && full

    /** A [width] × [height] picture scaled to fit [max] on its long side, never up; at least 1 × 1. */
    fun fit(width: Int, height: Int, max: Int): Pair<Int, Int> {
        if (width <= 0 || height <= 0) return 1 to 1
        val long = maxOf(width, height)
        if (long <= max) return width to height
        return maxOf(1, width * max / long) to maxOf(1, height * max / long)
    }
}
