package dev.pocketdesk.android.automation

/**
 * A session's ups and downs as [PhoneEvents]: started once it is up, ended
 * once when it goes down or the screen is left, never twice in a row.
 * [mode] is the session's kind (`screen`, `remote`).
 */
class SessionTell(private val mode: String) {
    private var up = false

    /** One event to tell, with its fields. */
    data class Told(val kind: PhoneEvents.Kind, val fields: Map<String, Any?>)

    /** The link is up: started, unless it was already. */
    @Synchronized
    fun up(): Told? {
        if (up) return null
        up = true
        return Told(PhoneEvents.Kind.SESSION_STARTED, mapOf("mode" to mode))
    }

    /** The link went down: ended (`lost`), if it was up. */
    @Synchronized
    fun lost(): Told? = end("lost")

    /** The screen was left: ended (`left`), if it was up. */
    @Synchronized
    fun left(): Told? = end("left")

    private fun end(reason: String): Told? {
        if (!up) return null
        up = false
        return Told(PhoneEvents.Kind.SESSION_ENDED, mapOf("reason" to reason))
    }
}
