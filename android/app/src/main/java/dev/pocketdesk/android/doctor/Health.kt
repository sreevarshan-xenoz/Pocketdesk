package dev.pocketdesk.android.doctor

/**
 * Something on in Pocketdesk's settings that cannot work, because Android,
 * an update or the owner took away what it needs (D1). In the order Home
 * lists them.
 */
enum class HealthProblem {
    /** A feature that runs on the notification listener is on, and notification access is not granted. */
    NOTIFICATION_ACCESS,

    /** Access is granted, but the listener has not connected, even after a rebind was asked. */
    LISTENER_STOPPED,

    /** "Show texts" without RECEIVE_SMS. */
    TEXTS_RECEIVE,

    /** "Reply to texts" without SEND_SMS. */
    TEXTS_SEND,

    /** "Names from my contacts" without READ_CONTACTS. */
    CONTACTS,

    /** Screenshots or recent photos to the computer without photo access. */
    PHOTOS,

    /** A feature that shows the phone a notification, and the app may not post any. */
    APP_NOTIFICATIONS,

    /** "Let the computer control this phone" with its Accessibility service off. */
    CONTROL_SERVICE,

    /** The same under Advanced Protection, which allows the service only to accessibility tools. */
    CONTROL_BLOCKED,

    /** The chosen folder for files from computers, whose grant is gone. */
    DOWNLOAD_FOLDER,
}

/** What the check reads, kept apart from Android so it is tested. */
data class HealthSnapshot(
    /** Whether any feature that runs on the notification listener is on. */
    val listenerFeatures: Boolean,
    val listenerAccess: Boolean,
    /** Whether the listener has connected; null while a rebind's answer is awaited. */
    val listenerConnected: Boolean?,
    val textsShow: Boolean,
    val receiveSms: Boolean,
    val textsReply: Boolean,
    val sendSms: Boolean,
    val contactNames: Boolean,
    val readContacts: Boolean,
    /** Whether screenshots or recent photos to the computer are on. */
    val photoFeatures: Boolean,
    val photoAccess: Boolean,
    /** Whether a feature that posts a notification on the phone is on (ring, files, links). */
    val postingFeatures: Boolean,
    val mayPost: Boolean,
    val control: Boolean,
    val controlService: Boolean,
    val advancedProtection: Boolean,
    val downloadFolderLost: Boolean,
)

object Health {
    fun problems(s: HealthSnapshot): List<HealthProblem> = buildList {
        if (s.listenerFeatures) {
            if (!s.listenerAccess) {
                add(HealthProblem.NOTIFICATION_ACCESS)
            } else if (s.listenerConnected == false) {
                add(HealthProblem.LISTENER_STOPPED)
            }
        }
        if (s.textsShow && !s.receiveSms) add(HealthProblem.TEXTS_RECEIVE)
        if (s.textsReply && !s.sendSms) add(HealthProblem.TEXTS_SEND)
        if (s.contactNames && !s.readContacts) add(HealthProblem.CONTACTS)
        if (s.photoFeatures && !s.photoAccess) add(HealthProblem.PHOTOS)
        if (s.postingFeatures && !s.mayPost) add(HealthProblem.APP_NOTIFICATIONS)
        if (s.control && !s.controlService) {
            add(if (s.advancedProtection) HealthProblem.CONTROL_BLOCKED else HealthProblem.CONTROL_SERVICE)
        }
        if (s.downloadFolderLost) add(HealthProblem.DOWNLOAD_FOLDER)
    }

    /** A set of problems as one word, the same in any order: what "Not now" remembers. */
    fun key(problems: List<HealthProblem>): String = problems.map { it.name }.sorted().joinToString(",")

    /**
     * Whether this boot runs another system build than the last one seen.
     * Nothing seen yet is not an update: the build is recorded at every app
     * start and update too, so only a phone never opened since installing
     * misses its first update.
     */
    fun systemUpdated(last: String?, now: String?): Boolean = now != null && last != null && last != now

    /**
     * Whether an update's check posts its notification: only for a problem
     * the owner has not seen on Home nor put off, since one known before the
     * update is not news it caused.
     */
    fun notifyAfterUpdate(problems: List<HealthProblem>, seen: String?, dismissed: String?): Boolean {
        val known = names(seen) + names(dismissed)
        return problems.any { it.name !in known }
    }

    private fun names(key: String?): Set<String> = key?.split(',')?.filter { it.isNotEmpty() }?.toSet().orEmpty()

    /** Whether Home shows the card: problems, and not the set the owner put off. */
    fun shows(problems: List<HealthProblem>, dismissed: String?): Boolean =
        problems.isNotEmpty() && key(problems) != dismissed
}
