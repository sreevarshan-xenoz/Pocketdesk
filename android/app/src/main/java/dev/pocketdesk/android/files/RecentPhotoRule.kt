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
        /** A video, not a photo (T4). */
        val video: Boolean = false,
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

    /**
     * One answer: the photos it lists, whether older ones wait for another
     * page, and where that page starts when it is not the last one listed
     * (rows looked past but not listed; T4 review), else null.
     */
    data class Page(val listed: List<Row>, val more: Boolean, val next: Long? = null)

    /**
     * The page to answer from [rows] (the newest, or those older than the
     * last page's last photo, newest first by id, as the query reads them):
     * camera photos and videos, at most [MAX_PHOTOS]. More wait when camera
     * ones were left over, or the query stopped at its limit ([lookedAll]
     * false) before the phone's oldest. By id, so the next page starts
     * right after this one's last (T4).
     */
    fun page(rows: List<Row>, lookedAll: Boolean): Page {
        val camera = rows.filter(::camera).sortedByDescending { it.id }
        val listed = camera.take(MAX_PHOTOS)
        return when {
            camera.size > MAX_PHOTOS -> Page(listed, true)
            !lookedAll -> Page(listed, true, rows.minOfOrNull { it.id }?.takeIf { it != listed.lastOrNull()?.id })
            else -> Page(listed, false)
        }
    }

    /** The photos to list from [rows]: [page]'s, as the first page lists them. */
    fun listed(rows: List<Row>): List<Row> = page(rows, lookedAll = true).listed

    /** Ids remembered as listed, at most: ten pages and some (T4). */
    const val MAX_REMEMBERED = 400

    /**
     * The ids a fetch may name after an answer listing [listed]: a newest
     * page starts over, an older one adds to [remembered] (T4: a photo of
     * any page shown may be saved), at most [MAX_REMEMBERED].
     */
    fun remember(remembered: Set<Long>, listed: List<Long>, olderPage: Boolean): Set<Long> =
        ((if (olderPage) remembered.toList() else emptyList()) + listed).takeLast(MAX_REMEMBERED).toSet()

    /** Whether the computer may fetch photo [id]: only one a page shown listed. */
    fun mayFetch(id: Long, lastListed: Set<Long>): Boolean = id in lastListed

    /** The sample size that brings a [width] by [height] image near [side] without going under it. */
    fun sampleSize(width: Int, height: Int, side: Int = THUMB_SIDE): Int {
        var sample = 1
        while (width / (sample * 2) >= side && height / (sample * 2) >= side) sample *= 2
        return sample
    }
}
