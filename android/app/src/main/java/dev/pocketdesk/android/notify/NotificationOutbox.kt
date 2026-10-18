package dev.pocketdesk.android.notify

/**
 * What the phone's notifications to the computer wait for, and what the
 * computer has been shown. Kept apart from Android so it is tested; used on
 * one thread.
 *
 * - While anything waits, a new notification joins the queue instead of
 *   overtaking it, so an older version can never land after a newer one.
 * - What the computer was shown is remembered apart from the queue, so a
 *   close on the phone reaches the computer even when a later update to
 *   that notification waited and expired.
 * - Each one is sent with how long it waited, in ms (0 when it went at
 *   once), so the computer can say it came late (C3).
 */
class NotificationOutbox<T>(
    private val pending: PendingNotifications<Item<T>> = PendingNotifications(),
    private val shownCapacity: Int = 500,
) {
    data class Item<T>(val pkg: String, val body: T)

    private val shown = LinkedHashSet<String>()

    /** How long notifications may wait for the computer, in ms. */
    var maxAgeMs: Long
        get() = pending.maxAgeMs
        set(value) {
            pending.maxAgeMs = value
        }

    /**
     * A notification to send. Returns the ids given up on (pushed out of a
     * full queue), for the caller's bookkeeping.
     */
    fun post(
        id: String,
        pkg: String,
        body: T,
        nowMs: Long,
        sendable: (id: String, pkg: String) -> Boolean,
        send: (T, waitedMs: Long) -> Boolean,
    ): List<String> {
        // "Don't wait" (C3): sent now or given up on, never queued; what
        // waited from before the setting changed is given up on first.
        if (pending.maxAgeMs <= 0) {
            val forgotten = flush(nowMs, sendable, send)
            if (send(body, 0)) {
                markShown(id)
                return forgotten
            }
            return forgotten + id
        }
        val othersWait = !pending.isEmpty()
        if (!othersWait && send(body, 0)) {
            markShown(id)
            return emptyList()
        }
        val gaveWay = listOfNotNull(pending.add(id, Item(pkg, body), nowMs))
        // Behind what waited: those go first, if the computer answers now.
        return gaveWay + if (othersWait) flush(nowMs, sendable, send) else emptyList()
    }

    /**
     * Send what waited, oldest first, until one fails. Returns the ids given
     * up on: expired, or no longer allowed ([sendable] false).
     */
    fun flush(
        nowMs: Long,
        sendable: (id: String, pkg: String) -> Boolean,
        send: (T, waitedMs: Long) -> Boolean,
    ): List<String> {
        if (pending.isEmpty()) return emptyList()
        val due = pending.due(nowMs)
        val forgotten = due.expired.toMutableList()
        for ((id, item) in due.send) {
            if (!sendable(id, item.pkg)) {
                pending.remove(id)
                forgotten += id
                continue
            }
            if (!send(item.body, due.waited[id] ?: 0)) break
            pending.sent(id)
            markShown(id)
        }
        return forgotten
    }

    /**
     * [id] was closed on the phone. Whatever of it still waits is dropped;
     * returns whether the computer was shown it, and so must close it too.
     */
    fun closed(id: String): Boolean {
        pending.remove(id)
        return shown.remove(id)
    }

    fun isEmpty(): Boolean = pending.isEmpty()

    private fun markShown(id: String) {
        shown.remove(id)
        shown.add(id)
        while (shown.size > shownCapacity) shown.remove(shown.first())
    }
}
