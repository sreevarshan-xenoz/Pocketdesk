package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.ComputerState
import kotlin.math.roundToInt

/**
 * The state widget's choices (G1), kept apart from Android so they are
 * tested: when to ask the computer, the numbers it shows and the line
 * saying when they were read.
 */
object StateWidgetPlan {
    /** Why the widget is being drawn. */
    enum class Why { Tap, Period }

    /**
     * Whether to ask the computer: always for a tap or when it was never
     * [tried] (a widget just placed, a new last computer); for the system's
     * period only when the owner lets it refresh by itself.
     */
    fun ask(why: Why, tried: Boolean, auto: Boolean): Boolean =
        why == Why.Tap || !tried || auto

    /** Whether the computer was asked already: it answered, or it failed. */
    fun tried(json: String?, failed: Boolean): Boolean = json != null || failed

    /** The widget's numbers, whole percents; a part not read is null. */
    data class Numbers(val cpu: Int?, val memory: Int?, val disk: Int?, val battery: Int?)

    fun numbers(state: ComputerState.State): Numbers = Numbers(
        cpu = state.cpu?.roundToInt()?.coerceIn(0, 100),
        memory = state.memory?.let { percent(it) },
        disk = state.disks.maxOfOrNull { percent(it) },
        battery = state.batteryCharge,
    )

    private fun percent(space: ComputerState.Space): Int = (space.fraction * 100).roundToInt()

    /** What the time line says. */
    enum class When { Updated, StaleSince, NotAnswering, Asking }

    fun timeLine(readAt: Long?, failed: Boolean, asking: Boolean): When = when {
        asking -> When.Asking
        readAt != null && !failed -> When.Updated
        readAt != null -> When.StaleSince
        failed -> When.NotAnswering
        else -> When.Asking
    }

    /** A kept reading is shown only while its computer is still the last one. */
    fun keeps(readFor: String?, lastComputer: String?): Boolean =
        readFor != null && readFor == lastComputer
}
