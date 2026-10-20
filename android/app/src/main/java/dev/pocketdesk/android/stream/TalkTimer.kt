package dev.pocketdesk.android.stream

import kotlin.math.abs

/**
 * The talk timer in Remote (H2): a countdown for a presenter, with buzzes
 * at five minutes left, one minute left and the end. Kept apart from
 * Android so it is tested; Remote only counts and buzzes.
 */
object TalkTimer {
    /** The lengths offered, in minutes. */
    val MINUTES = listOf(5, 10, 15, 20, 30, 45, 60)

    /** Seconds left as "12:34", or past the end as "+1:02". */
    fun text(leftSec: Long): String {
        val over = leftSec < 0
        val s = abs(leftSec)
        return (if (over) "+" else "") + "${s / 60}:${(s % 60).toString().padStart(2, '0')}"
    }

    /**
     * The warnings for a talk of [totalSec]: seconds left and how many
     * buzzes. A warning as long as the talk itself is left out.
     */
    fun marks(totalSec: Long): List<Pair<Long, Int>> =
        listOf(300L to 1, 60L to 2, 0L to 3).filter { (at, _) -> at == 0L || at < totalSec }

    /**
     * How many buzzes to give as the time left goes from [before] to [now]
     * seconds: the latest mark crossed, or 0.
     */
    fun pulses(totalSec: Long, before: Long, now: Long): Int = crossed(totalSec, before, now)?.second ?: 0

    /** The latest mark crossed going from [before] to [now], with its buzzes, or null. */
    fun crossed(totalSec: Long, before: Long, now: Long): Pair<Long, Int>? =
        marks(totalSec).filter { (at, _) -> before > at && now <= at }.minByOrNull { it.first }

    /**
     * Whole seconds left at [nowMs] for a talk ending at [endsAtMs], rounded
     * up, so the end mark comes at the end, not a second early (H2 review).
     */
    fun left(endsAtMs: Long, nowMs: Long): Long = -Math.floorDiv(nowMs - endsAtMs, 1000L)
}
