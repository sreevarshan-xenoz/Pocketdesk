package dev.pocketdesk.android.notify

/**
 * Which of a notification's buttons the computer is offered, and whether
 * an answer from it runs (T10c). Pure, for JVM tests.
 */
object NotificationActionRule {
    /**
     * What a `Notification.Action` is, for the choice: its title, whether it
     * takes typed text, and whether the computer may run it at all ([usable]
     * is false for one that needs the phone unlocked, has no intent, or
     * offers only fixed choices).
     */
    data class ActionInfo(val title: String?, val takesText: Boolean, val usable: Boolean = true)

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
            .filter { it.usable && !it.takesText }
            .mapNotNull { it.title }
            .filter(::usableTitle)
            .distinct()
            .take(MAX_ACTIONS)
            .toList()

    /** The index of the first button that takes text, if any. */
    fun replyIndex(actions: List<ActionInfo>): Int? = actions.indexOfFirst { it.usable && it.takesText }.takeIf { it >= 0 }

    /** Whether an answer from [host]:[port] is from the computer the phone sends to. */
    fun fromOwnComputer(host: String, port: Int, ownAddress: String?, ownPort: Int?): Boolean =
        ownAddress != null && host == ownAddress && port == ownPort

    /**
     * What a notification offers, as one string: a change to it is a change
     * to send, even when the title and text stay the same.
     */
    fun signature(offered: List<String>, reply: Boolean): String =
        if (offered.isEmpty() && !reply) "" else offered.joinToString("\u0000") + if (reply) "\u0000reply" else ""

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

    /** What the phone tells its computer of an answer (E1): handed to the app, or why not. */
    data class Outcome(val ok: Boolean, val reason: String?)

    /** The packet that says it (core's `NOTIFICATION_ANSWERED`). */
    const val ANSWERED = "kdeconnect.pocketdesk.notification_answered"

    /**
     * The outcome of an answer not run, in the words the computer shows;
     * none for one from another computer, which is not told (it is not the
     * phone's own), or for one that runs.
     */
    fun notRun(decision: Decision): Outcome? =
        when (decision) {
            Decision.Run, Decision.NotOwnComputer -> null
            Decision.Off -> Outcome(false, "answering is off on the phone")
            Decision.Gone -> Outcome(false, "the notification is gone from the phone")
            Decision.NotOffered -> Outcome(false, "the notification no longer offers that answer")
            Decision.TooMany -> Outcome(false, "too many answers at once")
        }

    /**
     * The outcome of an answer run: handed to the app, or [failed]; the app
     * [refused] it when its `PendingIntent` was cancelled.
     */
    fun ran(failed: Boolean, refused: Boolean): Outcome =
        when {
            !failed -> Outcome(true, null)
            refused -> Outcome(false, "the app refused it")
            else -> Outcome(false, "the phone could not run it")
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
