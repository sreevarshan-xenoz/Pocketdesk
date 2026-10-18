package dev.pocketdesk.android.notify

/**
 * Do Not Disturb kept in step with the computer (D2). Android's interruption
 * filter, as [android.app.NotificationManager]'s INTERRUPTION_FILTER_*
 * values, kept here so the decisions are tested.
 */
object QuietPlan {
    const val UNKNOWN = 0
    const val ALL = 1
    const val PRIORITY = 2
    const val NONE = 3
    const val ALARMS = 4

    /** The longest mute the computer may ask for: a day. */
    const val MAX_MINUTES = 1_440

    /** The computer's minutes, 0 to end a mute; null for a value out of range. */
    fun minutes(asked: Int): Int? = asked.takeIf { it in 0..MAX_MINUTES }

    /** The computer's mute: until when (wall clock), and the filter it set. */
    data class Mute(val untilMs: Long, val filter: Int)

    enum class Next {
        /** Not yet, or nothing to do. */
        WAIT,

        /** Its time came, and the filter is still the mute's: put "all" back. */
        END,

        /** The owner changed the filter meanwhile: forget the mute, leave theirs. */
        FORGET,
    }

    fun next(mute: Mute?, nowMs: Long, filter: Int): Next = when {
        mute == null || filter == UNKNOWN -> Next.WAIT
        filter != mute.filter -> Next.FORGET
        nowMs >= mute.untilMs -> Next.END
        else -> Next.WAIT
    }

    /** What the phone tells its computer. */
    data class State(val quiet: Boolean, val untilUnix: Long?, val byComputer: Boolean)

    fun state(filter: Int, mute: Mute?): State {
        val quiet = filter != ALL && filter != UNKNOWN
        val mine = quiet && mute != null && mute.filter == filter
        return State(quiet, if (mine) mute!!.untilMs / 1_000 else null, mine)
    }
}

object QuietRule {
    /**
     * Whether a notification stays on the phone: the setting [on], the phone
     * in Do Not Disturb ([filter] not "all"), and Android itself hiding it
     * ([matches] false). A priority contact's message still goes.
     */
    fun holdBack(on: Boolean, filter: Int, matches: Boolean): Boolean =
        on && filter != QuietPlan.ALL && filter != QuietPlan.UNKNOWN && !matches
}
