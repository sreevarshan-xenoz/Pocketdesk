package dev.pocketdesk.android.stream

/**
 * Frames fed to the decoder whose pictures have not come out yet, oldest
 * first, by the time each went in (microseconds).
 *
 * The decoder pairs each picture with its feed to time the decode, and polls
 * for frames briefly while a picture is still due, so a frame is drawn as
 * soon as it is decoded instead of when the next frame arrives (a second
 * later on a still desktop). A feed older than [staleUs] is taken as one the
 * decoder dropped: it is forgotten rather than waited for, so one lost frame
 * cannot keep the polling short. Kept apart from Android so it is tested.
 */
class PendingOutputs(
    private val staleUs: Long,
    private val max: Int,
) {
    private val fedAt = ArrayDeque<Long>()

    val size: Int get() = fedAt.size

    /** A frame went into the decoder at [at]. */
    fun fed(at: Long) {
        // A real-time decoder never holds this many frames: the pairing with
        // the output has been lost, so the oldest goes.
        if (fedAt.size >= max) fedAt.removeFirst()
        fedAt.addLast(at)
    }

    /** A picture came out: when its frame went in, or null if none is known. */
    fun out(): Long? = fedAt.removeFirstOrNull()

    /**
     * Whether a picture is still due at [now]. Feeds older than the stale
     * limit are forgotten first.
     */
    fun due(now: Long): Boolean {
        while (fedAt.isNotEmpty() && now - fedAt.first() > staleUs) fedAt.removeFirst()
        return fedAt.isNotEmpty()
    }

    /** Forget every feed: a new run of the codec cannot produce their pictures. */
    fun clear() = fedAt.clear()
}
