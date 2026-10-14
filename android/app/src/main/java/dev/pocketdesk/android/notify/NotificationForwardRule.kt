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
    /** `Notification.visibility`: [NotificationForwardRule.VISIBILITY_SECRET] and so on. */
    val visibility: Int,
    val publicTitle: String?,
    val publicText: String?,
)

/** What the desktop is sent for one notification. */
data class Outgoing(val id: String, val app: String, val title: String, val text: String)

/**
 * Which of the phone's notifications go to the desktop, and as what (N5a).
 *
 * Never this app's own, ongoing ones (music, navigation, calls, services),
 * group summaries, ones with nothing to show, or an excluded app's. The
 * lock screen's rule is the desktop's too: a secret one is not sent, and a
 * private one is sent as its public version, or as only that there is one.
 * An update that changes nothing shown (a progress bar) is not sent again.
 */
class NotificationForwardRule(private val ownPackage: String) {
    // Insertion-ordered, so the oldest is dropped first.
    private val sent = LinkedHashMap<String, Pair<String, String>>()

    fun decide(note: PostedNote, enabled: Boolean, excluded: Set<String>): Outgoing? {
        if (!enabled || note.pkg == ownPackage || note.ongoing || note.groupSummary) return null
        if (note.pkg in excluded || note.visibility == VISIBILITY_SECRET) return null
        val (title, text) = shown(note) ?: return null
        if (sent[note.key] == title to text) return null
        sent.remove(note.key)
        sent[note.key] = title to text
        while (sent.size > REMEMBERED) sent.remove(sent.keys.first())
        return Outgoing(note.key, note.appLabel, title, text)
    }

    /** The notification is gone: the same key posted again is sent again. */
    fun removed(key: String) {
        sent.remove(key)
    }

    private fun shown(note: PostedNote): Pair<String, String>? {
        if (note.visibility == VISIBILITY_PRIVATE) {
            val title = note.publicTitle?.trim().orEmpty()
            val text = note.publicText?.trim().orEmpty()
            return if (title.isEmpty() && text.isEmpty()) HIDDEN_TITLE to "" else title.ifEmpty { note.appLabel } to text
        }
        val title = note.title?.trim().orEmpty()
        val text = note.text?.trim().orEmpty()
        if (title.isEmpty() && text.isEmpty()) return null
        return title.ifEmpty { note.appLabel } to text
    }

    companion object {
        // Notification.VISIBILITY_*, repeated so JVM tests need no android.jar.
        const val VISIBILITY_SECRET = -1
        const val VISIBILITY_PRIVATE = 0
        const val VISIBILITY_PUBLIC = 1

        /** The title of a private notification that has no public version. */
        const val HIDDEN_TITLE = "New notification"

        /** How many notifications' last sent text is remembered. */
        const val REMEMBERED = 200
    }
}
