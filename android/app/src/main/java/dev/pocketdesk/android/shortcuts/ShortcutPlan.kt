package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.HostStore

/**
 * Which shortcuts outside the app to offer (F11): connecting to each saved
 * computer, most recent first, then waking the ones with a Wake-on-LAN
 * address, within what a launcher shows.
 */
object ShortcutPlan {
    enum class Kind { CONNECT, WAKE }

    data class Entry(val kind: Kind, val address: String) {
        /** Stable per computer and action, so a launcher keeps a pinned one. */
        val id: String
            get() = "${kind.name.lowercase()}:$address"
    }

    /** Launchers show four app shortcuts. */
    const val MAX = 4

    fun plan(hosts: List<HostStore.Host>, hasWake: (String) -> Boolean, max: Int = MAX): List<Entry> {
        val connect = hosts.map { Entry(Kind.CONNECT, it.address) }
        val wake = hosts.filter { hasWake(it.address) }.map { Entry(Kind.WAKE, it.address) }
        return (connect + wake).take(max)
    }

    /**
     * The saved computer a shortcut, the tile or the widget names. Only a
     * computer already saved: the intent can come from any app, and must
     * not add one.
     */
    fun savedHost(address: String?, hosts: List<HostStore.Host>): HostStore.Host? =
        address?.let { wanted -> hosts.firstOrNull { it.address == wanted } }

    /**
     * Whether files shared with these URI [schemes] may be sent: only what
     * another app shares through a provider (`content`). A `file` path is
     * opened with this app's own rights, which reach its keys and pairings.
     */
    fun shareable(schemes: List<String?>): Boolean = schemes.all { it == "content" }

    /** The category that makes a connect shortcut a Direct Share target (N2). */
    const val SHARE_CATEGORY = "dev.pocketdesk.android.category.SHARE_TO_COMPUTER"

    /**
     * The saved computer a Direct Share target names (N2): the share sheet
     * gives the shortcut's id, and only a connect shortcut of a computer
     * still saved is one. Any other id goes nowhere, never to another.
     */
    fun shortcutHost(shortcutId: String, hosts: List<HostStore.Host>): Share {
        val prefix = Entry(Kind.CONNECT, "").id
        if (!shortcutId.startsWith(prefix)) return Share.NotSaved
        return savedHost(shortcutId.removePrefix(prefix), hosts)?.let { Share.To(it) } ?: Share.NotSaved
    }

    /** Where a share goes, or why it goes nowhere. */
    sealed interface Share {
        data class To(val host: HostStore.Host) : Share

        /** No computer is saved. */
        data object NoneSaved : Share

        /** The computer named is not saved. */
        data object NotSaved : Share

        /** Another app named a computer, and the person has not let apps choose. */
        data object NotAllowed : Share
    }

    /**
     * Where a share goes (T7b): the last computer used, or the saved one
     * [address] names when [othersMayChoose] (Settings, off by default:
     * any app can start a share, and choosing the computer is more than the
     * share sheet lets it do). A named computer is never swapped for
     * another: refused when not saved, or when choosing is not allowed.
     */
    fun shareHost(address: String?, hosts: List<HostStore.Host>, othersMayChoose: Boolean): Share =
        when {
            address == null -> hosts.firstOrNull()?.let { Share.To(it) } ?: Share.NoneSaved
            !othersMayChoose -> Share.NotAllowed
            else -> savedHost(address, hosts)?.let { Share.To(it) } ?: Share.NotSaved
        }
}
