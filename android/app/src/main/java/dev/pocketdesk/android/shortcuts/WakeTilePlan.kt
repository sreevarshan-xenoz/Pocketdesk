package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.wake.WakePlan

/**
 * The wake tile's subtitle (G7), kept apart from Android so it is tested:
 * whether a wake can go from here, and after a tap how it went, up to
 * whether the computer answered.
 */
object WakeTilePlan {
    /** How long a finished tap's outcome is said before the tile goes back to ready. */
    const val SAID_MS = 60_000L

    /** How long the tile watches for the computer to answer after a wake. */
    const val WATCH_MS = 60_000L

    /** Where a tap is: sending, waiting for the computer, or how it ended. */
    enum class Step(val running: Boolean) {
        Sending(true),
        Waiting(true),
        Awake(false),
        NoAnswer(false),
        Failed(false),
    }

    /** The last tap's [step], since [atMs] (the wall clock). */
    data class Send(val step: Step, val atMs: Long)

    enum class Said { NoComputer, Ready, NotYet, Off, Sending, Waiting, Awake, NoAnswer, Failed }

    fun said(plan: WakePlan.Plan?, sent: Send?, nowMs: Long = 0L): Said = when {
        plan == null -> Said.NoComputer
        sent != null && (sent.step.running || nowMs - sent.atMs in 0..SAID_MS) -> when (sent.step) {
            Step.Sending -> Said.Sending
            Step.Waiting -> Said.Waiting
            Step.Awake -> Said.Awake
            Step.NoAnswer -> Said.NoAnswer
            Step.Failed -> Said.Failed
        }
        plan == WakePlan.Plan.Off -> Said.Off
        plan == WakePlan.Plan.NoMac || plan == WakePlan.Plan.NoRelay -> Said.NotYet
        // Away is ready too: a tap says why nothing goes from here (Waker.canWake).
        else -> Said.Ready
    }

    /** Whether a tap may start: never while one is under way (G7 review). */
    fun mayStart(sent: Send?): Boolean = sent == null || !sent.step.running

    /** An error short enough for a tile's subtitle, else null for the short words. */
    fun shortWhy(why: String?): String? = why?.takeIf { it.length <= 48 }
}
