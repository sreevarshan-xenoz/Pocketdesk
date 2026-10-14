package dev.pocketdesk.android.doctor

import androidx.annotation.StringRes
import dev.pocketdesk.android.R

/** How a check came out. */
enum class Status { GOOD, WARN, BAD, INFO }

/** What a finding's button does, if it has one. */
enum class FixAction {
    NONE,
    ASK_LOCAL_NETWORK,
    OPEN_WIFI_SETTINGS,
    OPEN_BATTERY_SETTINGS,
    OPEN_APP_SETTINGS,
    OPEN_DATA_SAVER,
    OPEN_NOTIFICATION_ACCESS,
    WAKE,
    PAIR_AGAIN,
}

/**
 * One row of the Connection Doctor (T1). [code] names the outcome for the
 * copied report, which never carries an address or an id; [message] and
 * [fix] are what the person reads.
 */
data class Finding(
    val code: String,
    val status: Status,
    @StringRes val message: Int,
    @StringRes val fix: Int? = null,
    val action: FixAction = FixAction.NONE,
)

/** The phone's own network, as the doctor needs it. */
enum class Link { NONE, WIFI, CELLULAR, ETHERNET, OTHER }

/** The phone's state, read once per run. */
data class PhoneState(
    val link: Link,
    val captivePortal: Boolean = false,
    val validated: Boolean = true,
    val hasIpv4: Boolean = true,
    val vpn: Boolean = false,
    /** Android 17's local network access; null before Android 17. */
    val localNetworkAllowed: Boolean? = null,
    /** The phone's IPv4 prefixes on Wi-Fi or Ethernet, as (address, length). */
    val lanPrefixes: List<Pair<String, Int>> = emptyList(),
    val batteryOptimized: Boolean = false,
    val backgroundRestricted: Boolean = false,
    val dataSaver: Boolean = false,
    /** Phone notifications, texts or auto-reconnect are on: background matters. */
    val backgroundFeaturesOn: Boolean = false,
    val notificationsAllowed: Boolean = true,
    /** Notification access, when "This phone's notifications" is on; else null. */
    val notificationAccess: Boolean? = null,
    /** The texts permission, when texts are on; else null. */
    val smsAllowed: Boolean? = null,
)

/** A TCP connect to the control port. */
enum class Tcp { OK, REFUSED, NO_ROUTE, NET_UNREACHABLE, DENIED, TIMEOUT, NO_NAME, NOT_RUN }

/** The compare-only TLS identity check. */
enum class Identity { MATCHES, CHANGED, NOT_PINNED, NOT_POCKETDESK, FAILED, NOT_RUN }

/** The UDP probe of the stream port. */
enum class Udp { ANSWERED, REFUSED, SILENT, NOT_RUN }

/** What the probes found. */
data class Probes(
    val tcp: Tcp,
    /** The same connect on a socket bound to Wi-Fi, run only when a VPN is on. */
    val tcpViaWifi: Tcp = Tcp.NOT_RUN,
    val identity: Identity = Identity.NOT_RUN,
    val udp: Udp = Udp.NOT_RUN,
)

/** What kind of address a computer is saved under. */
enum class AddressKind { PRIVATE_V4, TAILNET, LINK_LOCAL, PUBLIC, HOSTNAME, OTHER_V6 }

/** The Connection Doctor's judgement, kept apart from Android so it is tested. */
object ConnectionChecks {
    fun kindOf(address: String): AddressKind {
        val v4 = parseV4(address)
        if (v4 != null) {
            val (a, b) = (v4 ushr 24) to ((v4 ushr 16) and 0xff)
            return when {
                a == 10 || (a == 172 && b in 16..31) || (a == 192 && b == 168) -> AddressKind.PRIVATE_V4
                a == 100 && b in 64..127 -> AddressKind.TAILNET
                a == 169 && b == 254 -> AddressKind.LINK_LOCAL
                else -> AddressKind.PUBLIC
            }
        }
        val lower = address.lowercase()
        return when {
            lower.endsWith(".ts.net") -> AddressKind.TAILNET
            lower.startsWith("fd7a:115c:a1e0:") -> AddressKind.TAILNET
            lower.startsWith("fe80:") -> AddressKind.LINK_LOCAL
            ':' in lower -> AddressKind.OTHER_V6
            else -> AddressKind.HOSTNAME
        }
    }

    /** Whether IPv4 [address] is inside one of [prefixes]. */
    fun onSameNetwork(address: String, prefixes: List<Pair<String, Int>>): Boolean {
        val target = parseV4(address) ?: return false
        return prefixes.any { (net, length) ->
            val base = parseV4(net) ?: return@any false
            if (length !in 0..32) return@any false
            val mask = if (length == 0) 0 else -1 shl (32 - length)
            (base and mask) == (target and mask)
        }
    }

    /** The findings for one computer, in the order they are shown. */
    fun run(state: PhoneState, address: String, probes: Probes, canWake: Boolean): List<Finding> {
        val kind = kindOf(address)
        val out = mutableListOf<Finding>()
        out += network(state)
        state.localNetworkAllowed?.let { allowed ->
            out += if (allowed) {
                Finding("local_network_ok", Status.GOOD, R.string.doctor_local_network_ok)
            } else {
                Finding(
                    "local_network_denied", Status.BAD, R.string.doctor_local_network_denied,
                    R.string.doctor_local_network_denied_fix, FixAction.ASK_LOCAL_NETWORK,
                )
            }
        }
        vpn(state, kind, probes)?.let { out += it }
        out += sameNetwork(state, address, kind)
        out += answers(probes, canWake)
        out += identity(probes.identity)
        out += videoPort(probes)
        out += background(state)
        out += features(state)
        return out
    }

    private fun network(state: PhoneState): Finding = when {
        state.link == Link.NONE -> Finding(
            "offline", Status.BAD, R.string.doctor_offline, R.string.doctor_offline_fix, FixAction.OPEN_WIFI_SETTINGS,
        )
        state.captivePortal -> Finding(
            "captive_portal", Status.BAD, R.string.doctor_captive, R.string.doctor_captive_fix,
            FixAction.OPEN_WIFI_SETTINGS,
        )
        !state.hasIpv4 -> Finding("ipv6_only", Status.INFO, R.string.doctor_ipv6_only)
        !state.validated -> Finding("no_internet", Status.WARN, R.string.doctor_no_internet)
        state.link == Link.WIFI -> Finding("on_wifi", Status.GOOD, R.string.doctor_on_wifi)
        state.link == Link.CELLULAR -> Finding("on_mobile", Status.GOOD, R.string.doctor_on_mobile)
        else -> Finding("on_network", Status.GOOD, R.string.doctor_on_network)
    }

    private fun vpn(state: PhoneState, kind: AddressKind, probes: Probes): Finding? = when {
        kind == AddressKind.TAILNET && !state.vpn -> Finding(
            "tailscale_off", Status.BAD, R.string.doctor_tailscale_off, R.string.doctor_tailscale_off_fix,
        )
        kind == AddressKind.TAILNET -> Finding("tailscale_on", Status.GOOD, R.string.doctor_tailscale_on)
        !state.vpn -> null
        probes.tcp != Tcp.OK && probes.tcpViaWifi == Tcp.OK -> Finding(
            "vpn_captures_lan", Status.BAD, R.string.doctor_vpn_captures, R.string.doctor_vpn_captures_fix,
        )
        else -> Finding("vpn_on", Status.INFO, R.string.doctor_vpn_on)
    }

    private fun sameNetwork(state: PhoneState, address: String, kind: AddressKind): Finding = when {
        kind == AddressKind.TAILNET -> Finding("tailnet_address", Status.INFO, R.string.doctor_tailnet_address)
        kind == AddressKind.HOSTNAME -> Finding("saved_by_name", Status.INFO, R.string.doctor_saved_by_name)
        kind != AddressKind.PRIVATE_V4 -> Finding("not_lan_address", Status.INFO, R.string.doctor_not_lan_address)
        state.link == Link.CELLULAR -> Finding("on_mobile_home_address", Status.INFO, R.string.doctor_mobile_home_address)
        onSameNetwork(address, state.lanPrefixes) -> Finding("same_network", Status.GOOD, R.string.doctor_same_network)
        state.lanPrefixes.isEmpty() -> Finding("network_unknown", Status.INFO, R.string.doctor_network_unknown)
        else -> Finding(
            "other_network", Status.WARN, R.string.doctor_other_network, R.string.doctor_other_network_fix,
            FixAction.OPEN_WIFI_SETTINGS,
        )
    }

    private fun answers(probes: Probes, canWake: Boolean): Finding = when (probes.tcp) {
        Tcp.OK -> Finding("answers", Status.GOOD, R.string.doctor_answers)
        Tcp.REFUSED -> Finding("refused", Status.BAD, R.string.doctor_refused, R.string.doctor_refused_fix)
        Tcp.NO_ROUTE -> Finding(
            "no_route", Status.BAD, R.string.doctor_no_route,
            if (canWake) R.string.doctor_no_route_fix_wake else R.string.doctor_no_route_fix,
            if (canWake) FixAction.WAKE else FixAction.NONE,
        )
        Tcp.NET_UNREACHABLE -> Finding("net_unreachable", Status.BAD, R.string.doctor_net_unreachable)
        Tcp.DENIED -> Finding(
            "denied", Status.BAD, R.string.doctor_denied, R.string.doctor_local_network_denied_fix,
            FixAction.ASK_LOCAL_NETWORK,
        )
        Tcp.TIMEOUT -> Finding("timeout", Status.BAD, R.string.doctor_timeout, R.string.doctor_timeout_fix)
        Tcp.NO_NAME -> Finding("no_name", Status.BAD, R.string.doctor_no_name, R.string.doctor_no_name_fix)
        Tcp.NOT_RUN -> Finding("answers_not_checked", Status.INFO, R.string.doctor_not_checked)
    }

    private fun identity(identity: Identity): Finding = when (identity) {
        Identity.MATCHES -> Finding("key_matches", Status.GOOD, R.string.doctor_key_matches)
        Identity.CHANGED -> Finding(
            "key_changed", Status.BAD, R.string.doctor_key_changed, R.string.doctor_key_changed_fix,
            FixAction.PAIR_AGAIN,
        )
        Identity.NOT_PINNED -> Finding("not_paired", Status.INFO, R.string.doctor_not_paired)
        Identity.NOT_POCKETDESK -> Finding(
            "not_pocketdesk", Status.BAD, R.string.doctor_not_pocketdesk, R.string.doctor_not_pocketdesk_fix,
        )
        Identity.FAILED, Identity.NOT_RUN -> Finding("key_not_checked", Status.INFO, R.string.doctor_key_not_checked)
    }

    private fun videoPort(probes: Probes): Finding = when (probes.udp) {
        Udp.ANSWERED -> Finding("video_port_ok", Status.GOOD, R.string.doctor_video_ok)
        Udp.REFUSED -> Finding("video_port_closed", Status.BAD, R.string.doctor_video_closed, R.string.doctor_refused_fix)
        // UDP silence means something only when TCP got through.
        Udp.SILENT -> if (probes.tcp == Tcp.OK) {
            Finding("video_port_blocked", Status.BAD, R.string.doctor_video_blocked, R.string.doctor_video_blocked_fix)
        } else {
            Finding("video_port_not_checked", Status.INFO, R.string.doctor_video_not_checked)
        }
        Udp.NOT_RUN -> Finding("video_port_not_checked", Status.INFO, R.string.doctor_video_not_checked)
    }

    private fun background(state: PhoneState): List<Finding> {
        if (!state.backgroundFeaturesOn) return emptyList()
        val out = mutableListOf<Finding>()
        if (state.backgroundRestricted) {
            out += Finding(
                "background_restricted", Status.WARN, R.string.doctor_restricted, R.string.doctor_restricted_fix,
                FixAction.OPEN_APP_SETTINGS,
            )
        } else if (state.batteryOptimized) {
            out += Finding(
                "battery_optimized", Status.WARN, R.string.doctor_battery, R.string.doctor_battery_fix,
                FixAction.OPEN_BATTERY_SETTINGS,
            )
        }
        if (state.dataSaver) {
            out += Finding(
                "data_saver", Status.WARN, R.string.doctor_data_saver, R.string.doctor_data_saver_fix,
                FixAction.OPEN_DATA_SAVER,
            )
        }
        if (out.isEmpty()) out += Finding("background_ok", Status.GOOD, R.string.doctor_background_ok)
        return out
    }

    private fun features(state: PhoneState): List<Finding> {
        val out = mutableListOf<Finding>()
        if (!state.notificationsAllowed) {
            out += Finding(
                "notifications_off", Status.WARN, R.string.doctor_notifications_off, null, FixAction.OPEN_APP_SETTINGS,
            )
        }
        if (state.notificationAccess == false) {
            out += Finding(
                "notification_access_off", Status.WARN, R.string.doctor_notification_access_off,
                R.string.doctor_notification_access_fix, FixAction.OPEN_NOTIFICATION_ACCESS,
            )
        }
        if (state.smsAllowed == false) {
            out += Finding("texts_off", Status.WARN, R.string.doctor_texts_off, null, FixAction.OPEN_APP_SETTINGS)
        }
        return out
    }

    /** IPv4 dotted quad as an Int, or null. */
    private fun parseV4(text: String): Int? {
        val parts = text.split('.')
        if (parts.size != 4) return null
        var value = 0
        for (part in parts) {
            val n = part.toIntOrNull() ?: return null
            if (n !in 0..255 || part.length > 3) return null
            value = (value shl 8) or n
        }
        return value
    }
}

/** The copied report: what was checked and how it came out, never an address or id. */
object DoctorReport {
    fun text(findings: List<Finding>, android: String, manufacturer: String, app: String): String = buildString {
        appendLine("Pocketdesk connection check")
        appendLine("App $app · Android $android · $manufacturer")
        for (f in findings) appendLine("${f.status.name.lowercase()}: ${f.code}")
    }
}
