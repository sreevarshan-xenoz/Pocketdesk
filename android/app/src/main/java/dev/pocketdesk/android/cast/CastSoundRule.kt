package dev.pocketdesk.android.cast

/**
 * When "Show screen" also sends what the phone's apps play (K5): the
 * setting on, the microphone permission (Android asks it for capturing
 * other apps' sound), the computer saying it plays it, a screen (not the
 * camera), and Android 10 or later. Pure, for JVM tests.
 */
object CastSoundRule {
    /** Android 10, where other apps' sound may be captured. */
    const val MIN_SDK = 29

    fun capture(
        enabled: Boolean,
        recordGranted: Boolean,
        computerTakes: Boolean,
        camera: Boolean,
        sdk: Int,
    ): Boolean = enabled && recordGranted && computerTakes && !camera && sdk >= MIN_SDK

    /** Whether a cast's state (`castState`'s JSON) says the computer plays its sound. */
    fun computerTakes(castState: String): Boolean =
        runCatching { org.json.JSONObject(castState).optBoolean("sound", false) }.getOrDefault(false)
}
