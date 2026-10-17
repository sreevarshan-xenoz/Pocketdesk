package dev.pocketdesk.android.ui

import org.json.JSONObject

/** What the computer's answer to a privacy request says (U2). */
sealed interface PrivacyOutcome {
    /** Done: for a lock, the screen locked. */
    data object Done : PrivacyOutcome

    /** Not done, with the computer's reason when it gave one. */
    data class Refused(val why: String?) : PrivacyOutcome
}

/**
 * Reads `kdeconnect.pocketdesk.privacy` answers: `{ok, error?}` for the
 * request, and with `lock` asked, `locked` (the program that locked) or
 * `lockError`. A reply saying `ok: false` had counted as done, so a
 * refused grab showed privacy mode on and a refused lock said nothing.
 */
object PrivacyReply {
    fun outcome(
        ok: Boolean,
        error: String?,
        locked: String?,
        lockError: String?,
        askedLock: Boolean,
    ): PrivacyOutcome = when {
        !ok -> PrivacyOutcome.Refused(error)
        askedLock && locked == null -> PrivacyOutcome.Refused(lockError)
        else -> PrivacyOutcome.Done
    }

    /** [outcome] from the reply's JSON text; unreadable is refused. */
    fun read(json: String, askedLock: Boolean): PrivacyOutcome = runCatching {
        val reply = JSONObject(json)
        fun text(key: String) = reply.optString(key).ifBlank { null }
        outcome(
            ok = reply.optBoolean("ok", false),
            error = text("error"),
            locked = text("locked"),
            lockError = text("lockError"),
            askedLock = askedLock,
        )
    }.getOrElse { PrivacyOutcome.Refused(null) }
}
