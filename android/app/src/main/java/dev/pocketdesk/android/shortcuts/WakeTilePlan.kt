package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.wake.WakePlan

/**
 * The wake tile's subtitle (G7), kept apart from Android so it is tested:
 * whether a wake can go from here, and for a while after a tap how it went.
 */
object WakeTilePlan {
    /** How long a send's outcome is said before the tile goes back to ready. */
    const val SAID_MS = 60_000L

    /** The last tap's outcome: [ok], at [atMs] (the wall clock). */
    data class Send(val ok: Boolean, val atMs: Long)

    enum class Said { NoComputer, Ready, NotYet, Off, Sent, Failed }

    fun said(plan: WakePlan.Plan?, sent: Send?, nowMs: Long = 0L): Said = when {
        plan == null -> Said.NoComputer
        sent != null && nowMs - sent.atMs in 0..SAID_MS -> if (sent.ok) Said.Sent else Said.Failed
        plan == WakePlan.Plan.Off -> Said.Off
        plan == WakePlan.Plan.NoMac || plan == WakePlan.Plan.NoRelay -> Said.NotYet
        // Away is ready too: a tap says why nothing goes from here (Waker.canWake).
        else -> Said.Ready
    }
}
