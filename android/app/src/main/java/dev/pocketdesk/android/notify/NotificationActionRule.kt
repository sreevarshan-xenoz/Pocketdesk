package dev.pocketdesk.android.notify

/**
 * Which of a notification's buttons the computer is offered, and whether
 * an answer from it runs (T10c). Pure, for JVM tests.
 */
object NotificationActionRule {
    /** What a `Notification.Action` is, for the choice: its title and whether it takes text. */
    data class ActionInfo(val title: String?, val takesText: Boolean)

    /** Buttons offered, at most (core's `MAX_ACTIONS`). */
    const val MAX_ACTIONS = 3

    /** A title, at most, in characters (core's `MAX_ACTION_CHARS`). */
    const val MAX_TITLE_CHARS = 40

    /** Answers run per minute, at most. */
    const val PER_MINUTE = 10

    /** The plain buttons' titles, as offered: usable, distinct, first three. */
    fun offered(actions: List<ActionInfo>): List<String> =
        actions
            .asSequence()
            .filter { !it.takesText }
            .mapNotNull { it.title }
            .filter(::usableTitle)
            .distinct()
            .take(MAX_ACTIONS)
            .toList()

    /** The index of the first button that takes text, if any. */
    fun replyIndex(actions: List<ActionInfo>): Int? = actions.indexOfFirst { it.takesText }.takeIf { it >= 0 }

    private fun usableTitle(title: String): Boolean =
        title.isNotBlank() && title.length <= MAX_TITLE_CHARS && title.none { it.isISOControl() }

    enum class Decision { Run, Off, NotOwnComputer, Gone, NotOffered, TooMany }

    /**
     * Whether an answer runs: the setting on, from the phone's own computer,
     * for a notification still shown, with an answer it offered, within the
     * budget.
     */
    fun decide(settingOn: Boolean, ownComputer: Boolean, known: Boolean, offered: Boolean, withinBudget: Boolean): Decision =
        when {
            !settingOn -> Decision.Off
            !ownComputer -> Decision.NotOwnComputer
            !known -> Decision.Gone
            !offered -> Decision.NotOffered
            !withinBudget -> Decision.TooMany
            else -> Decision.Run
        }

    /** [PER_MINUTE] answers in any minute. */
    class Budget {
        private val times = ArrayDeque<Long>()

        /** Take one at [nowMs] (elapsed realtime), if the minute allows. */
        @Synchronized
        fun take(nowMs: Long): Boolean {
            while (times.isNotEmpty() && nowMs - times.first() >= 60_000L) times.removeFirst()
            if (times.size >= PER_MINUTE) return false
            times.addLast(nowMs)
            return true
        }
    }
}
