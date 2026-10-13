package dev.pocketdesk.android

import android.content.Context
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
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .remove("$KEY_WOL_MAC_PREFIX$address")
            .remove("$KEY_WAN_IDENTITY_PREFIX$address")
            .remove("$KEY_MONITOR_PREFIX$address")
            .remove("$KEY_PAIRED_PREFIX$address")
            .apply()
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
    }

    /** WoL MAC of the sleeping desktop behind [address] (Tier-2 #12 relay). */
    fun wolMac(ctx: Context, address: String): String =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("$KEY_WOL_MAC_PREFIX$address", "").orEmpty()

    fun saveWolMac(ctx: Context, address: String, mac: String) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("$KEY_WOL_MAC_PREFIX$address", mac)
            .apply()
    }

    private const val KEY_WOL_MAC_PREFIX = "wol_mac:"

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
    }

    private const val KEY_PAIRED_PREFIX = "paired_desktop:"
}
