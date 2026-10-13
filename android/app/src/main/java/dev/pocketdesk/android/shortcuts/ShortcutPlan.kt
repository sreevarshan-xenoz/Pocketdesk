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
}
