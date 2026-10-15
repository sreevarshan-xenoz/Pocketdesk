package dev.pocketdesk.android.stream

/**
 * When the video link counts as dead: no frame and no video byte for
 * [stallMs] (once a frame has come), or no frame at all for [firstFrameMs].
 *
 * Bytes count as well as frames because a large frame on a slow link (a
 * fresh full-size keyframe can be hundreds of KB) takes longer than
 * [stallMs] to arrive, and the link is alive all the while. Kept apart from
 * Android so it is tested.
 */
class StallWatch(
    private val stallMs: Long,
    private val firstFrameMs: Long,
    startedAt: Long,
) {
    private var sawFrame = false
    private var lastProgressAt = startedAt
    private var lastBytes = -1L

    /** A whole frame arrived at [at]. */
    fun frame(at: Long) {
        sawFrame = true
        lastProgressAt = at
    }

    /** Whether the link is dead at [at], given the video bytes received so far. */
    fun stalled(at: Long, videoBytes: Long): Boolean {
        if (lastBytes >= 0 && videoBytes != lastBytes) lastProgressAt = at
        lastBytes = videoBytes
        return at - lastProgressAt > if (sawFrame) stallMs else firstFrameMs
    }
}
