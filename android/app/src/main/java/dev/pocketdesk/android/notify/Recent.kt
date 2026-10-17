package dev.pocketdesk.android.notify

/**
 * A value worked out at most once per [ttlMs] (N7): the background timers
 * ask for the phone's own computer every few seconds, and working it out
 * reads and parses the saved computers each time. Thread-safe.
 */
class Recent<T>(private val ttlMs: Long) {
    private var value: T? = null
    private var at: Long? = null

    /** The value, worked out by [compute] if it is older than [ttlMs] at [nowMs]. */
    @Synchronized
    fun get(nowMs: Long, compute: () -> T): T {
        val since = at
        @Suppress("UNCHECKED_CAST")
        if (since != null && nowMs - since < ttlMs) return value as T
        val fresh = compute()
        value = fresh
        at = nowMs
        return fresh
    }

    /** Work it out again at the next [get]. */
    @Synchronized
    fun forget() {
        at = null
    }
}
