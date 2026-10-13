package dev.pocketdesk.android.stream

/**
 * The player's decisions, kept apart from MediaCodec and AudioTrack so they
 * can be tested (spec A5): a fixed latency budget, with no measured jitter
 * sizing or drift handling (roadmap 2792/2793). Owned by the playout thread;
 * not thread-safe.
 */
class PlayoutPolicy {
    private var lastSequence = -1L
    private var draining = false

    /**
     * Whether a packet is newer than every one played so far. One far
     * behind is a new audio task on the desktop counting from 0 again, not
     * a late packet.
     */
    fun accept(sequence: Long): Boolean {
        val restart = lastSequence - sequence > RESTART_GAP
        if (sequence <= lastSequence && !restart) return false
        lastSequence = sequence
        return true
    }

    /** Whether to write the next decoded frame, given how much is queued. */
    fun shouldWrite(queuedMs: Int): Boolean {
        if (draining && queuedMs <= START_MS) {
            draining = false
        } else if (!draining && queuedMs > CAP_MS) {
            draining = true
        }
        return !draining
    }

    /** Whether enough is queued to start (or restart) the track. */
    fun readyToPlay(queuedMs: Int): Boolean = queuedMs >= START_MS

    /** Whether a playing track ran dry and should pause to refill. */
    fun starved(playing: Boolean, queuedMs: Int): Boolean = playing && queuedMs == 0

    /** Forget a drain in progress (after a flush, the queue is empty). */
    fun rebuffer() {
        draining = false
    }

    companion object {
        const val START_MS = 60
        const val CAP_MS = 150

        /** How long one poll of the bridge waits for audio. */
        const val POLL_MS = 20

        /** 5 s of 20 ms packets: further back than any reorder. */
        const val RESTART_GAP = 250L

        /**
         * How long to sleep after a poll that took [elapsedMs]. An empty poll
         * that returned early (the session is gone, or the desktop closed it)
         * waits out the rest of its slot, so the playout thread cannot spin.
         */
        fun idlePauseMs(elapsedMs: Long, empty: Boolean): Long =
            if (empty) (POLL_MS - elapsedMs).coerceAtLeast(0) else 0

        private const val FRAMES_PER_MS = 48
        private const val MASK = 0xFFFF_FFFFL

        /**
         * Milliseconds queued in a track, from the frames written so far and
         * its playback head. Both are 32-bit counters that wrap (after
         * ~24.8 h at 48 kHz), so the difference is taken modulo 2^32.
         */
        fun queuedMs(framesWritten: Long, headPosition: Int): Int =
            (((framesWritten - (headPosition.toLong() and MASK)) and MASK) / FRAMES_PER_MS).toInt()
    }
}
