package dev.pocketdesk.android.stream

/**
 * The session's connection in one word (U4), shown as a coloured dot when
 * "Show connection details" is off: from the round trip, the end-to-end delay
 * and frames lost in the last few seconds. Not the frame rate: a still desktop
 * sends few frames on a perfect link.
 */
object LinkQuality {
    enum class Level { MEASURING, GOOD, FAIR, POOR }

    private const val GOOD_RTT_MS = 80
    private const val FAIR_RTT_MS = 200
    private const val GOOD_E2E_MS = 120
    private const val FAIR_E2E_MS = 250
    private const val FAIR_DROPS = 2
    private const val POOR_DROPS = 10

    /** [e2eMs] is 0 until the first frame arrives, and is then left out. */
    fun of(rttMs: Long, rttSamples: Int, e2eMs: Long, recentDrops: Long): Level {
        if (rttSamples == 0 && e2eMs == 0L) return Level.MEASURING
        val byRtt = when {
            rttSamples == 0 -> Level.GOOD
            rttMs <= GOOD_RTT_MS -> Level.GOOD
            rttMs <= FAIR_RTT_MS -> Level.FAIR
            else -> Level.POOR
        }
        val byDelay = when {
            e2eMs == 0L || e2eMs <= GOOD_E2E_MS -> Level.GOOD
            e2eMs <= FAIR_E2E_MS -> Level.FAIR
            else -> Level.POOR
        }
        val byDrops = when {
            recentDrops >= POOR_DROPS -> Level.POOR
            recentDrops >= FAIR_DROPS -> Level.FAIR
            else -> Level.GOOD
        }
        return maxOf(byRtt, byDelay, byDrops)
    }

    /**
     * Frames lost over the last [spanMs], from the session's running count.
     * The count starts again at zero in a new session, so one going backwards
     * clears what came before.
     */
    class DropWindow(private val spanMs: Long) {
        private val samples = ArrayDeque<Pair<Long, Long>>()

        fun add(atMs: Long, total: Long) {
            if (samples.isNotEmpty() && total < samples.last().second) samples.clear()
            samples.addLast(atMs to total)
            while (samples.size > 1 && samples.first().first < atMs - spanMs) samples.removeFirst()
        }

        fun recent(): Long =
            if (samples.isEmpty()) 0 else samples.last().second - samples.first().second
    }
}
