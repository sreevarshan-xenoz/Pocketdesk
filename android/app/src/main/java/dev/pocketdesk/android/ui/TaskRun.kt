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
    }

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
