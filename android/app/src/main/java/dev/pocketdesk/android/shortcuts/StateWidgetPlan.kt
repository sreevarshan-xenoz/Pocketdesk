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
     * period only when the owner lets it refresh by itself, and the network
     * is not [metered] (mobile data: G1 review).
     */
    fun ask(why: Why, tried: Boolean, auto: Boolean, metered: Boolean): Boolean =
        why == Why.Tap || !tried || (auto && !metered)

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

    /** How the widget draws its numbers (I2): bars with room for them, else one line. */
    enum class Layout { Line, Bars }

    /** The layout for a widget [heightDp] tall; 0 when the launcher never said. */
    fun layout(heightDp: Int): Layout = if (heightDp >= BARS_FROM_DP) Layout.Bars else Layout.Line

    /**
     * A widget's height as a launcher before Android 12 gives it: its
     * tallest held upright, its shortest on its side (I2 review).
     */
    fun heightDp(portrait: Boolean, minHeightDp: Int, maxHeightDp: Int): Int =
        if (portrait) maxHeightDp else minHeightDp

    /** What a bar shows. */
    enum class Part { Cpu, Memory, Disk, Battery }

    /** One bar: what it is and its whole percent. */
    data class Row(val part: Part, val percent: Int)

    /** The bars, one for each reading the computer gave, in a fixed order. */
    fun rows(numbers: Numbers): List<Row> = listOfNotNull(
        numbers.cpu?.let { Row(Part.Cpu, it) },
        numbers.memory?.let { Row(Part.Memory, it) },
        numbers.disk?.let { Row(Part.Disk, it) },
        numbers.battery?.let { Row(Part.Battery, it) },
    )

    /** Whether the time line is drawn as a warning: the computer did not answer. */
    fun warns(time: When): Boolean = time == When.NotAnswering || time == When.StaleSince

    /** From this height, four bars and the time line fit under the name. */
    const val BARS_FROM_DP = 110

    /** A kept reading is shown only while its computer is still the last one. */
    fun keeps(readFor: String?, lastComputer: String?): Boolean =
        readFor != null && readFor == lastComputer
}
