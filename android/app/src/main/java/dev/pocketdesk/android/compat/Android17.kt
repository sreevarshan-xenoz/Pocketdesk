package dev.pocketdesk.android.compat

/**
 * What Android 17 (API 37, which the app targets) changes for this app (N4,
 * the status history), as rules of the
 * SDK level so they are tested without a device.
 */
object Android17 {
    /** Android 17's API level. */
    const val SDK = 37

    /**
     * Whether the ring may raise the alarm volume. From 17 a volume set from
     * the background without a while-in-use foreground service is silently
     * ignored, and the ring runs from the notification listener: a ring
     * raised while the app was open would then never be put back.
     */
    fun ringRaisesVolume(sdk: Int): Boolean = sdk < SDK

    /**
     * Whether an input device drives the computer's pointer under capture.
     * Before 17 a captured touchpad sent absolute finger positions, so only
     * mice were captured; from 17 it reports like a captured mouse.
     */
    fun captures(sdk: Int, virtual: Boolean, mouse: Boolean, touchpad: Boolean): Boolean = when {
        virtual -> false
        sdk >= SDK -> mouse || touchpad
        else -> mouse && !touchpad
    }

    /** Whether Android holds texts with a one-time code from this app for 3 hours. */
    fun holdsCodeTexts(sdk: Int): Boolean = sdk >= SDK
}
