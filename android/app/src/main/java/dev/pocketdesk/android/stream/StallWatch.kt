package dev.pocketdesk.android.stream

/**
 * When the video link counts as dead: the connection has heard nothing from
 * the computer for [quietMs] (BG6), or no frame and no video byte for
 * [stallMs] (once a frame has come), or no frame at all for [firstFrameMs].
 *
 * The first is the quick one. The connection pings the computer every second
 * and its acknowledgement is what the phone hears, so a live link, a still
 * screen included (nothing to send), is never quiet for more than a moment,
 * and one that went away is quiet for good: noticed in [quietMs], not in
 * [stallMs].
 *
 * The others are the backstop, and the reason the quiet is read from the
 * connection and not from the picture. Bytes count as well as frames because
 * a large frame on a slow link (a fresh full-size keyframe can be hundreds of
 * KB) takes longer than [stallMs] to arrive, and the link is alive all the
 * while. Kept apart from Android so it is tested.
 */
class StallWatch(
    private val stallMs: Long,
    private val firstFrameMs: Long,
    startedAt: Long,
    private val quietMs: Long = Long.MAX_VALUE,
) {
    private var sawFrame = false
    private var lastProgressAt = startedAt
    private var lastBytes = -1L

    /** A whole frame arrived at [at]. */
    fun frame(at: Long) {
        sawFrame = true
        lastProgressAt = at
    }

    /**
     * Whether the link is dead at [at], given the video bytes received so far
     * and how long the connection has heard nothing ([linkQuietMs], 0 when it
     * cannot say).
     */
    fun stalled(at: Long, videoBytes: Long, linkQuietMs: Long = 0): Boolean {
        if (linkQuietMs > quietMs) return true
        if (lastBytes >= 0 && videoBytes != lastBytes) lastProgressAt = at
        lastBytes = videoBytes
        return at - lastProgressAt > if (sawFrame) stallMs else firstFrameMs
    }
}
