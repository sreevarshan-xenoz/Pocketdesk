package dev.pocketdesk.wear

import dev.pocketdesk.watchlink.WatchOp

/**
 * When the watch's own lock stands in front of a press, kept apart from
 * Android so it is tested. Music, a wake and a ring ask the phone for
 * what its owner can do from the phone's own tiles. A deck button may run
 * a task on the computer, and a question's No answers something the computer
 * is waiting on: those take a watch with a lock set that is unlocked now,
 * so a watch left on a table with no lock does not hand them to whoever
 * picks it up. (The phone keeps its own switches on top of this.)
 */
object WatchLockRule {
    enum class Verdict { Allowed, NoLockSet, Locked }

    fun needsLock(op: WatchOp): Boolean = op == WatchOp.DeckPress || op == WatchOp.AskDeny

    fun check(op: WatchOp, deviceSecure: Boolean, deviceLocked: Boolean): Verdict = when {
        !needsLock(op) -> Verdict.Allowed
        !deviceSecure -> Verdict.NoLockSet
        deviceLocked -> Verdict.Locked
        else -> Verdict.Allowed
    }
}
