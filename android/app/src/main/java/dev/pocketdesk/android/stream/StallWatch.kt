package dev.pocketdesk.android.stream

/** What the watch makes of the link: fine, quiet (worth a nudge and a word), or gone. */
enum class LinkState { Alive, Quiet, Dead }

/**
 * When the video link counts as quiet, and when as dead. It reads the
 * connection first and the picture second.
 *
 * The connection pings the computer every second and its acknowledgement is
 * what the phone hears, so a live link, a still screen included (nothing to
 * send), is never quiet for more than a moment. After [quietNoticeMs] of
 * silence the link is [LinkState.Quiet]: the stream says it is reconnecting
 * and keeps the last picture, the network is nudged, and nothing is torn
 * down, because a Wi-Fi roam, a burst of loss or a radio's power-save gap
 * is silent for a few seconds and the connection survives it (QUIC
 * migrates). Heard from again, it is [LinkState.Alive] and nothing happened.
 * Only after [quietDeadMs] is it [LinkState.Dead] and the session ended, to
 * come back through the race of the home and internet dials.
 *
 * The picture's own watch stays as the backstop: no frame and no video byte
 * for [stallMs] (once a frame has come), or no frame at all for
 * [firstFrameMs], is dead whatever the connection says. Bytes count as well
 * as frames because a large frame on a slow link (a fresh full-size keyframe
 * can be hundreds of KB) takes longer than [stallMs] to arrive, and the link
 * is alive all the while. A closed connection ([closed]) ends it at once.
 * Kept apart from Android so it is tested.
 */
class StallWatch(
    private val stallMs: Long,
    private val firstFrameMs: Long,
    startedAt: Long,
    private val quietNoticeMs: Long = Long.MAX_VALUE,
    private val quietDeadMs: Long = Long.MAX_VALUE,
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
     * The link's state at [at], given the video bytes received so far, how
     * long the connection has heard nothing ([linkQuietMs], 0 when it cannot
     * say), and whether it has closed.
     */
    fun check(at: Long, videoBytes: Long, linkQuietMs: Long = 0, closed: Boolean = false): LinkState {
        if (lastBytes >= 0 && videoBytes != lastBytes) lastProgressAt = at
        lastBytes = videoBytes
        return when {
            closed -> LinkState.Dead
            linkQuietMs > quietDeadMs -> LinkState.Dead
            at - lastProgressAt > if (sawFrame) stallMs else firstFrameMs -> LinkState.Dead
            linkQuietMs > quietNoticeMs -> LinkState.Quiet
            else -> LinkState.Alive
        }
    }

    /** Whether the link is dead at [at]: [check] says [LinkState.Dead]. */
    fun stalled(at: Long, videoBytes: Long, linkQuietMs: Long = 0): Boolean =
        check(at, videoBytes, linkQuietMs) == LinkState.Dead

    companion object {
        /** The connection heard nothing this long: say it is reconnecting, nudge the network. */
        const val QUIET_NOTICE_MS = 3_000L

        /** The connection heard nothing this long: the session ends and comes back by itself. */
        const val QUIET_DEAD_MS = 8_000L

        /** Frame gap that means the link is dead rather than the desktop idle: the backstop. */
        const val STALL_MS = 10_000L

        /** The wait for a session's first frame: the desk's 30 s approval, and margin. */
        const val FIRST_FRAME_MS = 45_000L

        /** The phone's watch, with the limits above. */
        fun forPhone(startedAt: Long) =
            StallWatch(STALL_MS, FIRST_FRAME_MS, startedAt, QUIET_NOTICE_MS, QUIET_DEAD_MS)
    }
}
