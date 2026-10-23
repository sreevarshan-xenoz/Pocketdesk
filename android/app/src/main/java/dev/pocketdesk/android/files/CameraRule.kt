package dev.pocketdesk.android.files

/**
 * Which new photos and videos go to the computer by themselves (T3). Pure,
 * for JVM tests.
 */
object CameraRule {
    /** Photos and videos one tick hands to the transfer service, at most; the rest wait for the next. */
    const val MAX_PER_TICK = 3

    /**
     * Whether [folder] (MediaStore's `RELATIVE_PATH`, or the file's folder
     * before Android 10) is the camera's: `DCIM/Camera`, in any case, or a
     * folder in it. Screenshots, screen recordings and other apps' images
     * under DCIM are not.
     */
    fun inCamera(folder: String?): Boolean {
        val names = folder?.split('/')?.filter { it.isNotEmpty() } ?: return false
        val dcim = names.indexOfFirst { it.equals("DCIM", ignoreCase = true) }
        return dcim >= 0 && names.getOrNull(dcim + 1)?.equals("Camera", ignoreCase = true) == true
    }

    /**
     * Whether one taken at [addedSecs] goes: taken since the switch was
     * turned on ([sinceSecs]). No age limit: one taken while the computer
     * was away goes when it is back, as a backup would.
     */
    fun due(addedSecs: Long, sinceSecs: Long): Boolean = addedSecs >= sinceSecs

    /** How long an empty photo or video is waited for, as one still being written. */
    const val EMPTY_WAIT_SECS = 600L

    /**
     * Whether an empty one added at [addedSecs] may still be written at
     * [nowSecs] (an app that writes after adding it, without marking it
     * pending): it waits that long, then is passed.
     */
    fun mayStillBeWritten(addedSecs: Long, nowSecs: Long): Boolean = nowSecs - addedSecs < EMPTY_WAIT_SECS

    /** Whether it may go on this network: anywhere, or on Wi-Fi only (the default). */
    fun mayGo(wifiOnly: Boolean, onWifi: Boolean): Boolean = !wifiOnly || onWifi
}
