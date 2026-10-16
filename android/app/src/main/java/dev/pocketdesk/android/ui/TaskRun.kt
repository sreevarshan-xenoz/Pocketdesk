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

        /** The start never left the phone: why. */
        data class NotSent(val why: String) : Status

        /**
         * Nothing came back in time: the computer may not let this phone
         * start tasks (it drops such a start without a word), or went away.
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
                    else -> Status.Unknown
                }
            else -> Status.Waiting(asks)
        }

    /** Whether the run is over, so the app stops asking for its result. */
    fun finished(status: Status): Boolean = status !is Status.Waiting && status != Status.Running
}
