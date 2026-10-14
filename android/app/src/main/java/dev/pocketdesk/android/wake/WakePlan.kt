package dev.pocketdesk.android.wake

/** How the phone wakes a computer (N6), the user's choice per computer. */
enum class WakeMethod {
    /** On this network, and from outside when an outside port is known. */
    AUTO,

    /** Broadcasts on this network only. */
    LAN,

    /** Ask another, always-on computer to send it. */
    RELAY,

    /** No Wake button. */
    OFF,
}

/** How the computer said it can be woken (`kdeconnect.pocketdesk.wake_info`). */
data class WakeInfo(val mac: String, val broadcast: String, val wired: Boolean, val port: Int)

/**
 * The user's wake settings for one computer. [outsidePort] 0 means the
 * port the computer announced; [mac] blank means the learned one.
 */
data class WakeSettings(
    val method: WakeMethod = WakeMethod.AUTO,
    val relay: String = "",
    val outsideHost: String = "",
    val outsidePort: Int = 0,
    val mac: String = "",
)

/** Where the phone sends a wake. Pure, for JVM tests. */
object WakePlan {
    /** The port a magic packet goes to on this network. */
    const val LAN_PORT = 9

    sealed interface Plan {
        /** Send the magic packet for [mac] to each `host:port` in [targets]. */
        data class Direct(val mac: String, val targets: List<String>) : Plan

        /** Ask the saved computer at [relay] to send it on its network. */
        data class Relay(val relay: String, val mac: String, val broadcast: String) : Plan

        data object Off : Plan

        /** No MAC known: the computer has not said, and none was typed. */
        data object NoMac : Plan

        /** "Through another computer" with none chosen. */
        data object NoRelay : Plan
    }

    private val MAC = Regex("^([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$")

    fun validMac(mac: String): Boolean = MAC.matches(mac.trim())

    /** An outside port field: empty (the announced one) or 1–65535. */
    fun validPort(text: String): Boolean = text.isBlank() || text.trim().toIntOrNull()?.let { it in 1..65535 } == true

    /**
     * The plan for one computer, from the user's [settings], what the
     * computer said ([learned]) and its last known addresses ([wanAddrs],
     * the WAN identity's `directAddrs`).
     */
    fun plan(settings: WakeSettings, learned: WakeInfo?, wanAddrs: List<String>): Plan {
        if (settings.method == WakeMethod.OFF) return Plan.Off
        val mac = settings.mac.trim().takeIf { validMac(it) } ?: learned?.mac?.takeIf { validMac(it) }
            ?: return Plan.NoMac
        val broadcast = learned?.broadcast?.takeIf { it.isNotBlank() }
        if (settings.method == WakeMethod.RELAY) {
            val relay = settings.relay.trim().takeIf { it.isNotEmpty() } ?: return Plan.NoRelay
            return Plan.Relay(relay, mac, broadcast.orEmpty())
        }
        val targets = mutableListOf<String>()
        broadcast?.let { targets += "$it:$LAN_PORT" }
        targets += "255.255.255.255:$LAN_PORT"
        if (settings.method == WakeMethod.AUTO) {
            val host = settings.outsideHost.trim().ifEmpty { publicIpv4(wanAddrs).orEmpty() }
            val port = settings.outsidePort.takeIf { it in 1..65535 } ?: learned?.port?.takeIf { it in 1..65535 }
            if (host.isNotEmpty() && port != null) targets += "$host:$port"
        }
        return Plan.Direct(mac, targets.distinct())
    }

    /**
     * The computer's public IPv4 address among its last known addresses
     * (`ip:port`): not private, shared (CGNAT and tailnets), link-local or
     * loopback, and not IPv6.
     */
    fun publicIpv4(addrs: List<String>): String? =
        addrs.firstNotNullOfOrNull { addr ->
            val ip = addr.substringBeforeLast(':')
            val parts = ip.split('.').mapNotNull { it.toIntOrNull()?.takeIf { n -> n in 0..255 } }
            if (parts.size != 4 || ip.count { it == '.' } != 3) return@firstNotNullOfOrNull null
            val (a, b) = parts
            val local =
                a == 10 || a == 127 || a == 0 ||
                    (a == 172 && b in 16..31) ||
                    (a == 192 && b == 168) ||
                    (a == 169 && b == 254) ||
                    (a == 100 && b in 64..127)
            ip.takeUnless { local }
        }

    /** Whether the phone is on the computer's network: one of its own subnets has the same broadcast. */
    fun atHome(learned: WakeInfo?, phoneBroadcasts: Set<String>): Boolean =
        learned != null && learned.broadcast in phoneBroadcasts
}
