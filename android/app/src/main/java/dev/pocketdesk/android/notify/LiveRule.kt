package dev.pocketdesk.android.notify

/**
 * Which ongoing notifications go to the computer as live updates, and
 * when (N4). Kept apart from Android so it is tested.
 */
object LiveRule {
    /** At most one update a notification this often. */
    const val MIN_GAP_MS = 5_000L

    /** A bar that moved less than this many percent is not sent. */
    const val STEP = 5

    /** What an ongoing notification shows: a bar, a timer, or both. */
    data class Live(
        val progress: Long,
        val max: Long,
        val indeterminate: Boolean,
        /** A timer's base, Unix ms; null without one. */
        val chronometer: Long?,
        val countdown: Boolean,
    ) {
        /** How far along, 0 to 100, for a measured bar. */
        val percent: Int?
            get() = if (max > 0 && !indeterminate) (progress.coerceIn(0, max) * 100 / max).toInt() else null
    }

    /**
     * What [ongoing] shows, or null when it is not one to send: Pocketdesk's
     * own ([own]), a media player's, a call, or one with neither a bar nor
     * a running timer.
     */
    fun read(
        ongoing: Boolean,
        own: Boolean,
        media: Boolean,
        call: Boolean,
        progress: Int,
        max: Int,
        indeterminate: Boolean,
        showChronometer: Boolean,
        whenMs: Long,
        countdown: Boolean,
    ): Live? {
        if (!ongoing || own || media || call) return null
        val bar = max > 0 || indeterminate
        val timer = showChronometer && whenMs > 0
        if (!bar && !timer) return null
        return Live(
            progress = progress.toLong().coerceAtLeast(0),
            max = max.toLong().coerceAtLeast(0),
            indeterminate = indeterminate,
            chronometer = if (timer) whenMs else null,
            countdown = timer && countdown,
        )
    }

    /** What was last sent for one notification, and when. */
    data class Sent(val atMs: Long, val live: Live, val title: String, val text: String)

    /**
     * When [live] with [title] and [text] is due at [nowMs] after [last]:
     * 0 for now, a wait in ms to send it then, or null for never (nothing
     * changed that a card shows). The first goes at once, and so does a
     * finished bar; others wait out [MIN_GAP_MS].
     */
    fun due(last: Sent?, live: Live, title: String, text: String, nowMs: Long): Long? {
        if (last == null) return 0
        val percent = live.percent
        val lastPercent = last.live.percent
        val moved = when {
            percent != null && lastPercent != null -> kotlin.math.abs(percent - lastPercent) >= STEP
            else -> percent != lastPercent
        }
        val changed = title != last.title || text != last.text || moved ||
            live.indeterminate != last.live.indeterminate ||
            live.chronometer != last.live.chronometer || live.countdown != last.live.countdown ||
            (percent == 100 && lastPercent != 100)
        if (!changed) return null
        if (percent == 100) return 0
        val gap = nowMs - last.atMs
        return if (gap >= MIN_GAP_MS) 0 else MIN_GAP_MS - gap
    }
}
