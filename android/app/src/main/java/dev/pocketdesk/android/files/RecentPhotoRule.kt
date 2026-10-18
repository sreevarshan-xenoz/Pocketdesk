package dev.pocketdesk.android.files

/**
 * Which of the phone's photos the computer sees (C7): the newest camera
 * photos, and only those. Kept apart from Android so it is tested.
 */
object RecentPhotoRule {
    /** Photos one answer carries, at most (the computer's bound too). */
    const val MAX_PHOTOS = 24

    /** A thumbnail's longest side, in pixels. */
    const val THUMB_SIDE = 256

    /** A thumbnail's bytes, at most (the computer's bound too). */
    const val MAX_THUMB_BYTES = 48 * 1024

    /** One MediaStore image row, as far as the rule cares. */
    data class Row(
        val id: Long,
        val name: String,
        /** Its folder below the storage's root, e.g. `DCIM/Camera/`. */
        val folder: String?,
        /** When it was taken, ms since the epoch; 0 when unknown. */
        val takenMs: Long,
        /** When it was added, seconds since the epoch. */
        val addedSecs: Long,
        val size: Long,
        val pending: Boolean,
    ) {
        /** Its time: when taken, else when added. */
        val timeMs: Long get() = if (takenMs > 0) takenMs else addedSecs * 1000
    }

    /**
     * Whether [row] is a camera photo: in `DCIM/`, whole, with a name; not in
     * `DCIM/Screenshots` (some makers keep screenshots there) or a hidden
     * folder such as `DCIM/.thumbnails` (C7 review).
     */
    fun camera(row: Row): Boolean {
        if (row.pending || row.name.isBlank() || row.name.startsWith(".")) return false
        val parts = row.folder?.trim('/')?.split('/') ?: return false
        return parts.firstOrNull().equals("DCIM", ignoreCase = true) &&
            parts.none { it.startsWith(".") } &&
            parts.getOrNull(1)?.equals("Screenshots", ignoreCase = true) != true
    }

    /** The photos to list from [rows]: camera photos, newest first, at most [MAX_PHOTOS]. */
    fun listed(rows: List<Row>): List<Row> =
        rows.filter(::camera).sortedByDescending { it.timeMs }.take(MAX_PHOTOS)

    /** Whether the computer may fetch photo [id]: only one the last answer listed. */
    fun mayFetch(id: Long, lastListed: Set<Long>): Boolean = id in lastListed

    /** The sample size that brings a [width] by [height] image near [side] without going under it. */
    fun sampleSize(width: Int, height: Int, side: Int = THUMB_SIDE): Int {
        var sample = 1
        while (width / (sample * 2) >= side && height / (sample * 2) >= side) sample *= 2
        return sample
    }
}
