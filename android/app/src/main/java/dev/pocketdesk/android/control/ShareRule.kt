package dev.pocketdesk.android.control

/**
 * A file dropped on the phone's screen window, to be shared into the app
 * in front (Apps part 2). The file comes by the ordinary transfer and the
 * ask to share it by the phone's action packet; either may arrive first,
 * so each waits for the other here. Pure, for JVM tests; [ShareDrops] does
 * the work.
 */
object ShareRule {
    /** How long either waits for the other. */
    const val WAIT_MS = 2 * 60_000L

    /** The most of each kept waiting. */
    const val MAX_WAITING = 8

    /** Windows that are never a target: the system's own. */
    private val NEVER = setOf("com.android.systemui")

    /** The ask: [name] and [size] name the file, [pkg] is the app in front when it came. */
    data class Pending(val address: String, val name: String, val size: Long, val pkg: String?, val at: Long)

    /** A file that came whole: where it is kept ([uri], none on Android 9 and before) and its type. */
    data class Arrived(
        val address: String,
        val name: String,
        val size: Long,
        val uri: String?,
        val mime: String?,
        val at: Long,
    )

    /** Whether [arrived] is the file [pending] asked about, from the same computer. */
    fun matches(pending: Pending, arrived: Arrived): Boolean =
        pending.address == arrived.address && pending.name == arrived.name && pending.size == arrived.size

    /**
     * The app to share into when the file is here: the one in front when
     * the ask came ([asked]), but only if it is still the one in front
     * ([now]); otherwise none, and the file stays in Downloads. The window
     * may have changed in the minutes the file took.
     */
    fun frontStill(asked: String?, now: String?): String? =
        asked?.takeIf { it.isNotBlank() && it == now }

    /** What to do with a file that came whole. */
    enum class Verdict {
        /** Start Android's share into the app in front. */
        SHARE,

        /** No app in front that can be told, or none the phone lists to open. */
        NO_APP,

        /** The app in front takes no file of this type (or the file has no address another app may read). */
        CANNOT_TAKE,
    }

    /**
     * The app in front is [pkg] (null when it could not be read): never
     * Pocketdesk ([own]) or the system's own windows, and only an app with a launcher
     * entry ([launchable], the phone's list of apps); then [hasUri] (the file has an
     * address other apps may open) and [takes] (the app handles `ACTION_SEND` for the
     * file's type).
     */
    fun verdict(pkg: String?, own: String, launchable: Boolean, hasUri: Boolean, takes: Boolean): Verdict = when {
        pkg.isNullOrBlank() || pkg == own || pkg in NEVER || !launchable -> Verdict.NO_APP
        !hasUri || !takes -> Verdict.CANNOT_TAKE
        else -> Verdict.SHARE
    }

    private val MIME = Regex("^[a-z0-9][a-z0-9.+-]*/[a-z0-9][a-z0-9.+-]*$")

    /** The type the share names: the file's own when it is a plain `type/subtype`, else a generic one. */
    fun mimeOf(raw: String?): String {
        val clean = raw?.trim()?.lowercase().orEmpty()
        return if (MIME.matches(clean)) clean else "application/octet-stream"
    }

    /** The name a computer gives a file to share: one line, no path, at most 255 characters. */
    fun validName(name: String): Boolean =
        name.isNotEmpty() && name.length <= 255 && name.none { it.isISOControl() || it == '/' || it == '\\' }

    /**
     * What waits: asks and files that have not met. [now] is a clock in ms.
     * A thread-safe meeting place: [offer] with an ask, [arrive] with a file;
     * each returns the other if it was already there (and fresh).
     */
    class Queue(private val now: () -> Long) {
        private val pending = ArrayDeque<Pending>()
        private val arrived = ArrayDeque<Arrived>()

        private fun drop(at: Long) = now() - at > WAIT_MS

        @Synchronized
        fun offer(ask: Pending): Arrived? {
            pending.removeAll { drop(it.at) }
            arrived.removeAll { drop(it.at) }
            val file = arrived.firstOrNull { matches(ask, it) }
            if (file != null) {
                arrived.remove(file)
                return file
            }
            // One ask a file: a newer one replaces an older.
            pending.removeAll { it.address == ask.address && it.name == ask.name && it.size == ask.size }
            pending.addLast(ask)
            while (pending.size > MAX_WAITING) pending.removeFirst()
            return null
        }

        @Synchronized
        fun arrive(file: Arrived): Pending? {
            pending.removeAll { drop(it.at) }
            arrived.removeAll { drop(it.at) }
            val ask = pending.firstOrNull { matches(it, file) }
            if (ask != null) {
                pending.remove(ask)
                return ask
            }
            arrived.addLast(file)
            while (arrived.size > MAX_WAITING) arrived.removeFirst()
            return null
        }
    }
}
