package dev.pocketdesk.android.notify

/**
 * Notifications that could not be sent yet (the phone asleep, the computer
 * out of reach), in memory only, so nothing a notification said is written
 * to disk. At most [capacity] wait, each for at most [maxAgeMs] (the
 * owner's setting, C3; an hour by default). Kept apart from Android so it
 * is tested; used on one thread.
 */
class PendingNotifications<T>(
    private val capacity: Int = 20,
    var maxAgeMs: Long = 60 * 60_000L,
) {
    /**
     * One waiting: its [body], when it first waited ([since], which keeps its
     * place and its age), and when this version of it was posted ([posted],
     * which how late it is counts from: an update posted a moment ago is
     * not late for its first version's wait).
     */
    private data class Waiting<T>(val body: T, val since: Long, val posted: Long)

    private val queue = LinkedHashMap<String, Waiting<T>>()

    /**
     * What to do now: the ones to send, the ones that expired unsent, and
     * how long each one to send has waited since this version of it was
     * posted, in ms (C3).
     */
    data class Due<T>(val send: List<Pair<String, T>>, val expired: List<String>, val waited: Map<String, Long>)

    /**
     * Wait to send [body] for [id]. An update to one already waiting keeps
     * its place and its age. Returns the id that gave way when full.
     */
    fun add(id: String, body: T, nowMs: Long): String? {
        val since = queue[id]?.since ?: nowMs
        queue[id] = Waiting(body, since, nowMs)
        if (queue.size <= capacity) return null
        val oldest = queue.keys.first()
        queue.remove(oldest)
        return oldest
    }

    /** Forget [id], closed on the phone before it went out; whether it was waiting. */
    fun remove(id: String): Boolean = queue.remove(id) != null

    /** [id] went out. */
    fun sent(id: String) {
        queue.remove(id)
    }

    /** Drop the expired ones, and list the rest in the order they came. */
    fun due(nowMs: Long): Due<T> {
        val expired = queue.filterValues { nowMs - it.since >= maxAgeMs }.keys.toList()
        expired.forEach { queue.remove(it) }
        return Due(
            queue.map { (id, w) -> id to w.body },
            expired,
            queue.mapValues { (_, w) -> nowMs - w.posted },
        )
    }

    fun waiting(): List<Pair<String, T>> = queue.map { (id, w) -> id to w.body }

    fun isEmpty(): Boolean = queue.isEmpty()
}
