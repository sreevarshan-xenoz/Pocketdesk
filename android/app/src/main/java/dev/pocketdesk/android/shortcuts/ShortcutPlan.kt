package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.HostStore

/**
 * Which shortcuts outside the app to offer (F11): connecting to each saved
 * computer, most recent first, then waking the ones with a Wake-on-LAN
 * address, within what a launcher shows.
 */
object ShortcutPlan {
    enum class Kind { CONNECT, WAKE }

    /**
     * One shortcut. A connect shortcut is also a Direct Share target (N2),
     * so its id carries this install's [token]: shortcut ids are readable
     * only by the system and the launcher, so another app cannot forge one
     * to choose a computer (N2 review).
     */
    data class Entry(val kind: Kind, val address: String, val token: String = "", val pairedId: String? = null) {
        /**
         * Stable per computer and action, so a launcher keeps a pinned one:
         * a paired computer's names its paired id (AZ1: by its address, a
         * move turned it off and another computer saved there took it over).
         */
        val id: String
            get() {
                val target = pairedId?.let { "$PAIRED$it" } ?: address
                return if (kind == Kind.CONNECT && token.isNotEmpty()) {
                    "connect:$token:$target"
                } else {
                    "${kind.name.lowercase()}:$target"
                }
            }
    }

    /** Before a paired id in a shortcut id; no address starts with it. */
    private const val PAIRED = "paired:"

    /**
     * What a shortcut id names: a paired computer (AZ1), or an address (a
     * computer never paired, or a shortcut made before AZ1).
     */
    sealed interface Target {
        data class Paired(val id: String) : Target

        data class Address(val address: String) : Target
    }

    fun targetOf(id: String): Target {
        val rest = addressOf(id)
        return if (rest.startsWith(PAIRED)) Target.Paired(rest.removePrefix(PAIRED)) else Target.Address(rest)
    }

    fun kindOf(id: String): Kind = if (id.startsWith("wake:")) Kind.WAKE else Kind.CONNECT

    /** A connect shortcut's token: 16 hex digits, which no address starts with before a colon. */
    private val TOKEN = Regex("^[0-9a-f]{16}:")

    /** The address a shortcut id names, with or without a token. */
    fun addressOf(id: String): String {
        val rest = id.substringAfter(':')
        return if (id.startsWith("connect:") && TOKEN.containsMatchIn(rest)) rest.substring(17) else rest
    }

    /** Launchers show four app shortcuts. */
    const val MAX = 4

    fun plan(
        hosts: List<HostStore.Host>,
        hasWake: (String) -> Boolean,
        max: Int = MAX,
        token: String = "",
        pairedId: (String) -> String? = { null },
    ): List<Entry> {
        val connect = hosts.map { Entry(Kind.CONNECT, it.address, token, pairedId(it.address)) }
        val wake = hosts.filter { hasWake(it.address) }.map { Entry(Kind.WAKE, it.address, pairedId = pairedId(it.address)) }
        return (connect + wake).take(max)
    }

    /**
     * The saved computer a shortcut, the tile or the widget names. Only a
     * computer already saved: the intent can come from any app, and must
     * not add one. One made for a paired computer, [pairedFor], names it
     * wherever it is now, never another computer saved at its old address
     * (AZ1).
     */
    fun savedHost(
        address: String?,
        hosts: List<HostStore.Host>,
        pairedFor: String? = null,
        pairedId: (String) -> String? = { null },
    ): HostStore.Host? =
        if (pairedFor != null) {
            hosts.firstOrNull { pairedId(it.address) == pairedFor }
        } else {
            address?.let { wanted -> hosts.firstOrNull { it.address == wanted } }
        }

    /** The saved computer a shortcut id names (AZ1). */
    private fun hostFor(id: String, hosts: List<HostStore.Host>, pairedId: (String) -> String?): HostStore.Host? =
        when (val target = targetOf(id)) {
            is Target.Paired -> savedHost(null, hosts, target.id, pairedId)
            is Target.Address -> savedHost(target.address, hosts)
        }

    /** What becomes of a pinned shortcut at a refresh (AZ1). */
    sealed interface Pinned {
        data object Keep : Pinned

        /** Its computer moved: pointed at [host]'s address. */
        data class Move(val host: HostStore.Host) : Pinned

        /** Its computer is no longer saved. */
        data object Off : Pinned
    }

    /**
     * What becomes of the pinned shortcut [id], whose intent names
     * [address]: kept while its computer is saved there, moved with it, or
     * turned off (AZ1: one made for a removed computer must not wake for
     * another saved at its address).
     */
    fun pinnedFate(
        id: String,
        address: String?,
        hosts: List<HostStore.Host>,
        pairedId: (String) -> String?,
    ): Pinned {
        val host = hostFor(id, hosts, pairedId) ?: return Pinned.Off
        return if (host.address == address) Pinned.Keep else Pinned.Move(host)
    }

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
     * gives the shortcut's id, and only a connect shortcut with this
     * install's [token], of a computer still saved, is one. Any other id
     * goes nowhere, never to another computer.
     */
    fun shortcutHost(
        shortcutId: String,
        hosts: List<HostStore.Host>,
        token: String,
        pairedId: (String) -> String? = { null },
    ): Share {
        if (token.isEmpty()) return Share.NotSaved
        if (!shortcutId.startsWith("connect:$token:")) return Share.NotSaved
        return hostFor(shortcutId, hosts, pairedId)?.let { Share.To(it) } ?: Share.NotSaved
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
