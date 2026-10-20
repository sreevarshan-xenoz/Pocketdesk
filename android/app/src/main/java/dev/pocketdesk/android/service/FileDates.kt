package dev.pocketdesk.android.service

/**
 * A file's date as it travels with it (K1): the `lastModified` of a share
 * or an offer, milliseconds since 1970. Pure, for JVM tests.
 */
object FileDates {
    /** What a share sends: the provider's date, or 0 (none) when off or unknown. */
    fun toSend(enabled: Boolean, modified: Long?): Long =
        if (enabled && modified != null && modified > 0) modified else 0L

    /** A browse listing's `modified`, in seconds, as an offer's date in ms; 0 for none. */
    fun fromListing(seconds: Long): Long = if (seconds > 0) seconds * 1000 else 0L

    /**
     * The date to give a file received from an offer saying [offered] (0 or
     * less for none), held to [now]; null when off or none.
     */
    fun toKeep(enabled: Boolean, offered: Long, now: Long): Long? =
        if (enabled && offered > 0) minOf(offered, now) else null
}
