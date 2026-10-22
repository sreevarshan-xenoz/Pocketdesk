package dev.pocketdesk.android.ui

/**
 * Named tasks (ecosystem 1c): what a run of one of the computer's tasks is
 * doing, from the computer's latest result for it. Pure, for JVM tests.
 */
object TaskRun {
    sealed interface Status {
        /** Sent; nothing back yet (the owner may be asked first). */
        data class Waiting(val asks: Boolean) : Status

        data object Running : Status

        /** It ended with this exit code (null: it could not start, or a signal ended it). */
        data class Done(val code: Int?) : Status

        data object StoppedAtDeadline : Status

        data object Denied : Status

        /** Already running, or two tasks are. */
        data object Busy : Status

        /** The computer has no such task (any more). */
        data object Unknown : Status

        /** "Start my tasks" is off for this phone on the computer (E7). */
        data object NotPermitted : Status

        /** The start never left the phone: why. */
        data class NotSent(val why: String) : Status

        /**
         * Nothing came back in time: the computer went away, or is from
         * before E7 and drops a start this phone may not make without a word.
         */
        data object NoAnswer : Status
    }

    /** How long a start may wait for its first answer: the owner's minute when it asks first. */
    fun answerWithinMs(asks: Boolean): Long = if (asks) 75_000 else 20_000

    /** [status], or [Status.NoAnswer] once a start still waiting has waited [waitedMs] too long. */
    fun givenUp(status: Status, waitedMs: Long): Status =
        if (status is Status.Waiting && waitedMs > answerWithinMs(status.asks)) Status.NoAnswer else status

    /**
     * The run's status from the result's fields ([state] null while no
     * result came); [asks] is whether the task asks its owner first.
     */
    fun status(state: String?, code: Int?, timedOut: Boolean, reason: String?, asks: Boolean): Status =
        when (state) {
            null -> Status.Waiting(asks)
            "running" -> Status.Running
            "done" -> if (timedOut) Status.StoppedAtDeadline else Status.Done(code)
            "refused" ->
                when (reason) {
                    "denied" -> Status.Denied
                    "busy" -> Status.Busy
                    "not_permitted" -> Status.NotPermitted
                    else -> Status.Unknown
                }
            else -> Status.Waiting(asks)
        }

    /**
     * How long a run is followed from outside a screen (O1 review): a link
     * that dropped after "running" brings no end, so the follow stops.
     */
    const val FOLLOW_MS = 15 * 60_000L

    /** [status], or [Status.NoAnswer] once a run not over was followed [followedMs] past [FOLLOW_MS]. */
    fun followedTooLong(status: Status, followedMs: Long): Status =
        if (!finished(status) && followedMs > FOLLOW_MS) Status.NoAnswer else status

    /** Whether the run is over, so the app stops asking for its result. */
    fun finished(status: Status): Boolean = status !is Status.Waiting && status != Status.Running

    /**
     * How a finished run is told to other apps (ecosystem 1f): its state,
     * and its code or why it was refused; never its output.
     */
    fun told(status: Status): Map<String, Any?> =
        when (status) {
            is Status.Done -> mapOf("state" to "done", "code" to status.code, "timed_out" to false)
            Status.StoppedAtDeadline -> mapOf("state" to "done", "timed_out" to true)
            Status.Denied -> mapOf("state" to "refused", "reason" to "denied")
            Status.Busy -> mapOf("state" to "refused", "reason" to "busy")
            Status.Unknown -> mapOf("state" to "refused", "reason" to "unknown")
            Status.NotPermitted -> mapOf("state" to "refused", "reason" to "not_permitted")
            is Status.NotSent -> mapOf("state" to "not_sent")
            Status.NoAnswer -> mapOf("state" to "no_answer")
            is Status.Waiting, Status.Running -> emptyMap()
        }
}
