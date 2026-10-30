package dev.pocketdesk.android

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import dev.pocketdesk.android.wake.WakeInfo
import dev.pocketdesk.android.wake.WakeMethod
import dev.pocketdesk.android.wake.WakePlan
import dev.pocketdesk.android.wake.WakeSettings
import org.json.JSONArray
import org.json.JSONObject

/**
 * Saved desktops + auto-connect toggle, persisted for Tier 1 #5. The list
 * lives as a JSON array (most-recent first); the single-host keys from the
 * pre-card-list UI are still read once for migration.
 */
object HostStore {
    data class Host(
        val address: String,
        val port: Int,
        val controlPort: Int = DEFAULT_CONTROL_PORT,
    )

    const val DEFAULT_CONTROL_PORT = 1716
    const val DEFAULT_STREAMING_PORT = 4716

    private const val PREFS = "pocketdesk-host"
    private const val KEY_ADDRESS = "address"
    private const val KEY_PORT = "port"
    private const val KEY_CONTROL_PORT = "control_port"
    private const val KEY_AUTO = "auto_connect"
    private const val KEY_HOSTS = "hosts"

    /** Saved desktops, most recently connected first. */
    fun hosts(ctx: Context): List<Host> {
        val raw = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString(KEY_HOSTS, null)
        if (raw != null) {
            val parsed = runCatching {
                val arr = JSONArray(raw)
                (0 until arr.length()).map { i ->
                    val o = arr.getJSONObject(i)
                    Host(
                        o.getString("address"),
                        o.optInt("port", DEFAULT_STREAMING_PORT),
                        o.optInt("control_port", DEFAULT_CONTROL_PORT),
                    )
                }
            }.getOrNull()
            if (parsed != null) return parsed
        }
        // Migration: the legacy single-host keys become the one entry.
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val legacy = prefs.getString(KEY_ADDRESS, null)?.takeIf { it.isNotBlank() }
            ?.let {
                Host(
                    it,
                    prefs.getInt(KEY_PORT, DEFAULT_STREAMING_PORT),
                    prefs.getInt(KEY_CONTROL_PORT, DEFAULT_CONTROL_PORT),
                )
            }
        return listOfNotNull(legacy)
    }

    fun lastHost(ctx: Context): Host? = hosts(ctx).firstOrNull()

    /** Upsert [host] and move it to the front (it is now the "last" host). */
    fun save(ctx: Context, host: Host) {
        val rest = hosts(ctx).filterNot { it.address == host.address }
        writeHosts(ctx, listOf(host) + rest)
    }

    /**
     * UX1: a session with the saved computer at [address] came up (its
     * stream, or a remote or sound session admitted), for its card's "Used
     * 2 hours ago"; again as it ends, and for a cast, camera or phone-sound
     * session too (UX2, review N10). Not on pairing, a shortcut or a tap that never connects
     * (PR #575 review N2). Nothing for an address that is not saved.
     */
    fun markUsed(ctx: Context, address: String, now: Long = System.currentTimeMillis()) {
        if (hosts(ctx).none { it.address == address }) return
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            putLong("$KEY_LAST_USED_PREFIX$address", now)
        }
    }

    /**
     * UX1: when [address] was last used, in ms since the epoch; null before,
     * and null for a time more than five minutes ahead of [now] (a clock set
     * forward, then back, would have pinned "Used just now"; review N1).
     */
    fun lastUsed(ctx: Context, address: String, now: Long = System.currentTimeMillis()): Long? =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getLong("$KEY_LAST_USED_PREFIX$address", 0L)
            .takeIf { it > 0L && it <= now + FUTURE_SLACK_MS }

    private const val FUTURE_SLACK_MS = 5 * 60_000L

    /**
     * Keep [host] in the list without moving it to the front: a computer
     * reached from discovery is saved only once it has paired, so an
     * unverified announcement never becomes the auto-connect host.
     */
    fun keep(ctx: Context, host: Host) {
        if (hosts(ctx).none { it.address == host.address }) {
            writeHosts(ctx, hosts(ctx) + host)
        }
    }

    /** Forget a desktop and every per-address record hanging off it. */
    fun remove(ctx: Context, address: String) {
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        // No other card's wake is sent through it any more (BC2), as a move
        // takes them along (AX2); before the card goes (review).
        rewriteRelays(prefs) { WakePlan.relayRemoved(it, address) }
        writeHosts(ctx, hosts(ctx).filterNot { it.address == address })
        prefs.edit {
            for (prefix in PER_ADDRESS) remove("$prefix$address")
        }
    }

    /** Each card's Wake settings that [change] gives new ones for, written. */
    private fun rewriteRelays(prefs: SharedPreferences, change: (WakeSettings) -> WakeSettings?) {
        prefs.edit(commit = true) {
            for ((key, value) in prefs.all) {
                if (!key.startsWith(KEY_WAKE_SETTINGS_PREFIX) || value !is String) continue
                val changed = wakeSettingsFrom(value)?.let(change) ?: continue
                putString(key, wakeSettingsJson(changed))
            }
        }
    }

    /** The records kept per saved address: removed with it, moved with it (O3). */
    private val PER_ADDRESS by lazy {
        listOf(
            KEY_WOL_MAC_PREFIX,
            KEY_WAKE_INFO_PREFIX,
            KEY_WAKE_SETTINGS_PREFIX,
            KEY_WAN_IDENTITY_PREFIX,
            KEY_MONITOR_PREFIX,
            KEY_PAIRED_PREFIX,
            KEY_NAME_PREFIX,
            KEY_CHOSEN_NAME_PREFIX,
            // Item 4: the release it said it runs.
            KEY_VERSION_PREFIX,
            // O6: the player chosen for the computer.
            KEY_PLAYER_PREFIX,
            // UX1: when it was last used.
            KEY_LAST_USED_PREFIX,
        )
    }

    /**
     * O3: the saved computer at [old] is the one now at [new] (it proved the
     * key [old] was pinned to there): its card takes the new address and
     * ports, in its place in the list, with every per-address record.
     * Nothing happens when [old] is not saved or [new] already is.
     */
    fun move(ctx: Context, old: String, new: Host): Boolean {
        val list = hosts(ctx)
        if (list.none { it.address == old } || list.any { it.address == new.address }) return false
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val all = prefs.all
        // The records first, the card last (O3 review: the card seen at its
        // new address without its pairing could be passed over meanwhile).
        prefs.edit(commit = true) {
            // What the computer taught for its old network (its broadcast)
            // is taught again at the next connection (O3 review).
            remove("$KEY_WAKE_INFO_PREFIX$old")
            for (prefix in PER_ADDRESS) {
                if (prefix == KEY_WAKE_INFO_PREFIX) continue
                when (val value = all["$prefix$old"]) {
                    is String -> putString("$prefix${new.address}", value)
                    is Int -> putInt("$prefix${new.address}", value)
                    is Long -> putLong("$prefix${new.address}", value)
                    is Boolean -> putBoolean("$prefix${new.address}", value)
                    else -> continue
                }
                remove("$prefix$old")
            }
        }
        // The wakes sent through it, the moved card's own too, go to the
        // new address (AX2).
        rewriteRelays(prefs) { WakePlan.relayMoved(it, old, new.address) }
        writeHosts(ctx, list.map { if (it.address == old) new else it })
        return true
    }

    /**
     * T1: the saved computer at [address] answers on another control port
     * now (it proved its pinned key there): its card takes it, in its place.
     * Nothing happens when it is not saved.
     */
    fun setControlPort(ctx: Context, address: String, controlPort: Int): Boolean {
        val list = hosts(ctx)
        if (list.none { it.address == address }) return false
        writeHosts(ctx, list.map { if (it.address == address) it.copy(controlPort = controlPort) else it })
        return true
    }

    private const val KEY_NAME_PREFIX = "computer_name:"
    private const val KEY_CHOSEN_NAME_PREFIX = "computer_name_chosen:"
    private const val KEY_PLAYER_PREFIX = "computer_player:"
    private const val KEY_VERSION_PREFIX = "computer_version:"
    private const val KEY_LAST_USED_PREFIX = "last_used:"

    /** O6: the player chosen for the computer at [address], or null for the computer's own choice. */
    fun player(ctx: Context, address: String): String? =
        dev.pocketdesk.android.ui.ComputerPlayers.named(stored(ctx, "$KEY_PLAYER_PREFIX$address"))

    /** O6: choose [id] as the computer's player at [address]; null goes back to the computer's choice. */
    fun setPlayer(ctx: Context, address: String, id: String?) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            when (val named = dev.pocketdesk.android.ui.ComputerPlayers.named(id)) {
                null -> remove("$KEY_PLAYER_PREFIX$address")
                else -> putString("$KEY_PLAYER_PREFIX$address", named)
            }
        }
    }

    private fun stored(ctx: Context, key: String): String? =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(key, null)?.takeIf { it.isNotBlank() }

    /** The owner's name for the computer at [address] (U3), or null. */
    fun chosenName(ctx: Context, address: String): String? = stored(ctx, "$KEY_CHOSEN_NAME_PREFIX$address")

    /** What the computer at [address] calls itself (U3), or null. */
    fun claimedName(ctx: Context, address: String): String? = stored(ctx, "$KEY_NAME_PREFIX$address")

    /** The computer's name if it has one: the owner's, else its own (U3). */
    fun name(ctx: Context, address: String): String? = chosenName(ctx, address) ?: claimedName(ctx, address)

    /** What the computer at [address] is shown by: its name, else its address. */
    fun shownName(ctx: Context, address: String): String =
        ComputerName.shown(chosenName(ctx, address), claimedName(ctx, address), address)

    /**
     * Keep what the computer at [address] calls itself (the pairing link,
     * then each connection). A saved computer only; the owner's rename is
     * kept beside it, never replaced. Whether it changed.
     */
    fun saveClaimedName(ctx: Context, address: String, raw: String?): Boolean {
        val name = ComputerName.clean(raw) ?: return false
        if (hosts(ctx).none { it.address == address } || claimedName(ctx, address) == name) return false
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            putString("$KEY_NAME_PREFIX$address", name)
        }
        return true
    }

    /** The Pocketdesk release the computer at [address] said it runs (item 4), or null. */
    fun claimedVersion(ctx: Context, address: String): String? = stored(ctx, "$KEY_VERSION_PREFIX$address")

    /**
     * Keep what the computer at [address] said of its release at its latest
     * identity ([PeerVersion.kept]): one that said none, or no version,
     * clears it, so a downgraded computer shows no old release; nothing
     * heard keeps it. A saved computer only. Whether it changed.
     */
    fun saveClaimedVersion(ctx: Context, address: String, heard: PeerVersion.Heard?): Boolean {
        if (hosts(ctx).none { it.address == address }) return false
        val old = claimedVersion(ctx, address)
        val new = PeerVersion.kept(heard, old)
        if (new == old) return false
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            when (new) {
                null -> remove("$KEY_VERSION_PREFIX$address")
                else -> putString("$KEY_VERSION_PREFIX$address", new)
            }
        }
        return true
    }

    /** The owner names the computer at [address]; blank goes back to its own. */
    fun rename(ctx: Context, address: String, raw: String?) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            when (val name = ComputerName.clean(raw)) {
                null -> remove("$KEY_CHOSEN_NAME_PREFIX$address")
                else -> putString("$KEY_CHOSEN_NAME_PREFIX$address", name)
            }
        }
    }

    private fun writeHosts(ctx: Context, list: List<Host>) {
        val arr = JSONArray()
        for (h in list) {
            arr.put(
                JSONObject()
                    .put("address", h.address)
                    .put("port", h.port)
                    .put("control_port", h.controlPort),
            )
        }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY_HOSTS, arr.toString())
            .apply()
        changed()
    }

    /** WoL MAC of the sleeping desktop behind [address] (Tier-2 #12 relay). */
    fun wolMac(ctx: Context, address: String): String =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("$KEY_WOL_MAC_PREFIX$address", "").orEmpty()

    /** The MAC typed for [address]: it becomes the one its wake settings use. */
    fun saveWolMac(ctx: Context, address: String, mac: String) {
        saveWakeSettings(ctx, address, wakeSettings(ctx, address).copy(mac = mac))
    }

    private const val KEY_WOL_MAC_PREFIX = "wol_mac:"
    private const val KEY_WAKE_INFO_PREFIX = "wake_info:"
    private const val KEY_WAKE_SETTINGS_PREFIX = "wake_settings:"

    /** How the computer at [address] said it can be woken (N6), or null. */
    fun wakeInfo(ctx: Context, address: String): WakeInfo? =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("$KEY_WAKE_INFO_PREFIX$address", null)
            ?.let { json ->
                runCatching {
                    val o = JSONObject(json)
                    WakeInfo(
                        mac = o.getString("mac"),
                        broadcast = o.optString("broadcast"),
                        wired = o.optBoolean("wired"),
                        port = o.optInt("port"),
                    )
                }.getOrNull()
            }

    /** Keep the `wake_info` body the computer sent; returns whether it changed. */
    fun saveWakeInfo(ctx: Context, address: String, json: String): Boolean {
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (prefs.getString("$KEY_WAKE_INFO_PREFIX$address", null) == json) return false
        prefs.edit { putString("$KEY_WAKE_INFO_PREFIX$address", json) }
        return true
    }

    /**
     * The user's wake settings for [address]. A MAC typed before N6 carries
     * over with what Wake did then: ask the saved computer itself, an
     * always-on one, to send it.
     */
    fun wakeSettings(ctx: Context, address: String): WakeSettings {
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val legacyMac = prefs.getString("$KEY_WOL_MAC_PREFIX$address", "").orEmpty()
        val json = prefs.getString("$KEY_WAKE_SETTINGS_PREFIX$address", null)
            ?: return legacyWakeSettings(address, legacyMac, wakeInfo(ctx, address)?.mac)
        return wakeSettingsFrom(json) ?: WakeSettings(mac = legacyMac)
    }

    /** Wake settings as [saveWakeSettings] keeps them, or null when they do not read. */
    private fun wakeSettingsFrom(json: String): WakeSettings? =
        runCatching {
            val o = JSONObject(json)
            WakeSettings(
                method = runCatching {
                    WakeMethod.valueOf(o.optString("method"))
                }.getOrDefault(WakeMethod.AUTO),
                relay = o.optString("relay"),
                outsideHost = o.optString("outsideHost"),
                outsidePort = o.optInt("outsidePort"),
                mac = o.optString("mac"),
            )
        }.getOrNull()

    private fun wakeSettingsJson(settings: WakeSettings): String =
        JSONObject()
            .put("method", settings.method.name)
            .put("relay", settings.relay)
            .put("outsideHost", settings.outsideHost)
            .put("outsidePort", settings.outsidePort)
            .put("mac", settings.mac)
            .toString()

    /**
     * Settings for a computer saved before N6 with a typed MAC: Wake asked
     * the saved computer to send it, so it stays a relay, unless the
     * computer itself announced that same MAC (then it is the one asleep,
     * and the phone wakes it directly).
     */
    private fun legacyWakeSettings(address: String, legacyMac: String, learnedMac: String?): WakeSettings {
        if (legacyMac.isBlank()) return WakeSettings()
        fun norm(mac: String) = mac.filter { it.isLetterOrDigit() }.uppercase()
        if (learnedMac != null && norm(learnedMac) == norm(legacyMac)) return WakeSettings(mac = legacyMac)
        return WakeSettings(method = WakeMethod.RELAY, relay = address, mac = legacyMac)
    }

    fun saveWakeSettings(ctx: Context, address: String, settings: WakeSettings) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            putString("$KEY_WAKE_SETTINGS_PREFIX$address", wakeSettingsJson(settings))
            putString("$KEY_WOL_MAC_PREFIX$address", settings.mac)
        }
    }

    fun autoConnect(ctx: Context): Boolean =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_AUTO, false)

    fun setAutoConnect(ctx: Context, enabled: Boolean) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putBoolean(KEY_AUTO, enabled)
            .apply()
    }

    /** Cached iroh WAN identity (`kdeconnect.pocketdesk.endpoint` body) per desktop address. */
    fun wanIdentity(ctx: Context, address: String): String? =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("$KEY_WAN_IDENTITY_PREFIX$address", null)?.takeIf { it.isNotBlank() }

    fun saveWanIdentity(ctx: Context, address: String, identityJson: String) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("$KEY_WAN_IDENTITY_PREFIX$address", identityJson)
            .apply()
    }

    private const val KEY_WAN_IDENTITY_PREFIX = "wan_identity:"

    /** Selected streaming monitor per desktop (`-1` = server default). */
    fun monitorIndex(ctx: Context, address: String): Int =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getInt("$KEY_MONITOR_PREFIX$address", -1)

    fun saveMonitorIndex(ctx: Context, address: String, index: Int) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putInt("$KEY_MONITOR_PREFIX$address", index)
            .apply()
    }

    private const val KEY_MONITOR_PREFIX = "monitor:"

    /** Device id of the desktop this phone paired with at [address] (Tier-2 #11b). */
    fun pairedDesktopId(ctx: Context, address: String): String? =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("$KEY_PAIRED_PREFIX$address", null)?.takeIf { it.isNotBlank() }

    fun savePairedDesktop(ctx: Context, address: String, serverId: String) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("$KEY_PAIRED_PREFIX$address", serverId)
            .apply()
        changed()
    }

    /**
     * Counts changes to the saved computers and their pairings, so what is
     * worked out from them and kept a while (the phone's own computer, N7)
     * is worked out again at once: a removed computer must not keep acting
     * as this phone's (N7 review).
     */
    val generation = java.util.concurrent.atomic.AtomicInteger()

    private fun changed() {
        generation.incrementAndGet()
    }

    private const val KEY_PAIRED_PREFIX = "paired_desktop:"
}
