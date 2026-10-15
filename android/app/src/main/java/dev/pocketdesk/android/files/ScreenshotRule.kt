package dev.pocketdesk.android.files

/**
 * Which new images go to the computer as screenshots (T10h). Pure, for JVM
 * tests.
 */
object ScreenshotRule {
    /** A screenshot older than this when it is seen (the computer was away) is not sent. */
    const val MAX_AGE_SECS = 600L

    /** A screenshot larger than this is not sent. */
    const val MAX_BYTES = 50L * 1024 * 1024

    /** Screenshots one tick sends, at most; the rest wait for the next. */
    const val MAX_PER_TICK = 5

    /**
     * Whether [folder] (MediaStore's `RELATIVE_PATH`, or the file's folder
     * before Android 10) is a screenshots folder: one of its names is
     * `Screenshots`, as in `DCIM/Screenshots/` or `Pictures/Screenshots/`.
     */
    fun inScreenshots(folder: String?): Boolean =
        folder?.split('/')?.any { it.equals("Screenshots", ignoreCase = true) } == true

    /**
     * Whether an image added at [addedSecs] is sent at [nowSecs]: taken
     * since the switch was turned on ([sinceSecs]), and not too long ago.
     */
    fun due(addedSecs: Long, sinceSecs: Long, nowSecs: Long): Boolean =
        addedSecs >= sinceSecs && nowSecs - addedSecs <= MAX_AGE_SECS

    fun sendable(size: Long): Boolean = size in 1..MAX_BYTES
}
