package dev.pocketdesk.android

import android.content.Context
import androidx.core.content.edit
import dev.pocketdesk.android.wake.WakeInfo
import dev.pocketdesk.android.wake.WakeMethod
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
        writeHosts(ctx, hosts(ctx).filterNot { it.address == address })
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            for (prefix in PER_ADDRESS) remove("$prefix$address")
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
        writeHosts(ctx, list.map { if (it.address == old) new else it })
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val all = prefs.all
        prefs.edit {
            for (prefix in PER_ADDRESS) {
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
        return true
    }

    private const val KEY_NAME_PREFIX = "computer_name:"
    private const val KEY_CHOSEN_NAME_PREFIX = "computer_name_chosen:"

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
        return runCatching {
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
        }.getOrDefault(WakeSettings(mac = legacyMac))
    }

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
        val json =
            JSONObject()
                .put("method", settings.method.name)
                .put("relay", settings.relay)
                .put("outsideHost", settings.outsideHost)
                .put("outsidePort", settings.outsidePort)
                .put("mac", settings.mac)
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit {
            putString("$KEY_WAKE_SETTINGS_PREFIX$address", json.toString())
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
