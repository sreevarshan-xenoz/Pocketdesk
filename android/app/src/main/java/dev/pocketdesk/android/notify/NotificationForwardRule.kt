package dev.pocketdesk.android.notify

/** A notification the phone posted, reduced to what [NotificationForwardRule] decides on. */
data class PostedNote(
    val key: String,
    val pkg: String,
    val appLabel: String,
    val ongoing: Boolean,
    val groupSummary: Boolean,
    val title: String?,
    val text: String?,
    /** The strictest lock-screen visibility: [NotificationForwardRule.effectiveVisibility]. */
    val visibility: Int,
    val publicTitle: String?,
    val publicText: String?,
    /** `FLAG_LOCAL_ONLY`: the app asked that it stay on this device. */
    val localOnly: Boolean,
    /** Posted in another profile (a work profile) or user. */
    val otherProfile: Boolean,
    val clearable: Boolean,
)

/** What the desktop is sent for one notification. */
data class Outgoing(val id: String, val app: String, val title: String, val text: String, val clearable: Boolean)

/**
 * Which of the phone's notifications go to the desktop, and as what (N5a).
 *
 * Only apps the user switched on. Never this app's own, ongoing ones
 * (music, navigation, calls, services), group summaries, ones with nothing
 * to show, ones the app keeps on this device (`FLAG_LOCAL_ONLY`), or another
 * profile's. The lock screen's rule is the desktop's too: a secret one is not
 * sent, and a private one is sent as its public version, or as only that
 * there is one. A one-time code is replaced. An update that changes nothing
 * shown (a progress bar) is not sent again.
 */
class NotificationForwardRule(private val ownPackage: String) {
    // Insertion-ordered, so the oldest is dropped first.
    private val sent = LinkedHashMap<String, Pair<String, String>>()

    fun decide(note: PostedNote, enabled: Boolean, shared: Set<String>): Outgoing? {
        if (!enabled || note.pkg == ownPackage || note.pkg !in shared) return null
        if (note.ongoing || note.groupSummary || note.localOnly || note.otherProfile) return null
        if (note.visibility == VISIBILITY_SECRET) return null
        val (title, text) = shown(note) ?: return null
        if (sent[note.key] == title to text) return null
        sent.remove(note.key)
        sent[note.key] = title to text
        while (sent.size > REMEMBERED) sent.remove(sent.keys.first())
        return Outgoing(note.key, note.appLabel, title, text, note.clearable)
    }

    /**
     * The notification is gone, or its send failed: the same key posted
     * again is sent again. Returns whether it had been sent, so the desktop
     * is told to close only notifications it was shown (N5c).
     */
    fun removed(key: String): Boolean = sent.remove(key) != null

    private fun shown(note: PostedNote): Pair<String, String>? {
        if (note.visibility == VISIBILITY_PRIVATE) {
            val title = note.publicTitle?.trim().orEmpty()
            val text = note.publicText?.trim().orEmpty()
            return if (title.isEmpty() && text.isEmpty()) HIDDEN_TITLE to "" else title.ifEmpty { note.appLabel } to text
        }
        val title = note.title?.trim().orEmpty()
        val text = note.text?.trim().orEmpty()
        if (title.isEmpty() && text.isEmpty()) return null
        return title.ifEmpty { note.appLabel } to if (looksLikeCode("$title $text")) CODE_HIDDEN else text
    }

    companion object {
        // Notification.VISIBILITY_*, repeated so JVM tests need no android.jar.
        const val VISIBILITY_SECRET = -1
        const val VISIBILITY_PRIVATE = 0
        const val VISIBILITY_PUBLIC = 1

        /** NotificationManager.VISIBILITY_NO_OVERRIDE. */
        const val NO_OVERRIDE = -1000

        /** The title of a private notification that has no public version. */
        const val HIDDEN_TITLE = "New notification"

        /** The text sent in place of a one-time code. */
        const val CODE_HIDDEN = "A code (kept on the phone)"

        /** How many notifications' last sent text is remembered. */
        const val REMEMBERED = 200

        private val CODE_WORDS = Regex("""\b(code|otp|passcode|password|pin|verification|verify|2fa|one[- ]time)\b""", RegexOption.IGNORE_CASE)
        private val CODE_DIGITS = Regex("""(?<![\d:.,])(?:[A-Z]-)?\d{4,8}(?![\d:.,]\d)""")

        /** Whether [text] carries a one-time code: 4 to 8 digits next to a word that names one. */
        fun looksLikeCode(text: String): Boolean = CODE_WORDS.containsMatchIn(text) && CODE_DIGITS.containsMatchIn(text)

        /**
         * The strictest of the notification's own visibility, its channel's
         * lock-screen setting, the user's override from the ranking, and the
         * system's "hide sensitive content" (which makes everything private
         * at most). Lower is stricter.
         */
        fun effectiveVisibility(own: Int, channel: Int, ranking: Int, systemHidesSensitive: Boolean): Int {
            var v = own
            if (channel != NO_OVERRIDE) v = minOf(v, channel)
            if (ranking != NO_OVERRIDE) v = minOf(v, ranking)
            if (systemHidesSensitive) v = minOf(v, VISIBILITY_PRIVATE)
            return v
        }
    }
}
