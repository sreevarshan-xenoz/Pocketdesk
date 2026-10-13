package dev.linuxlink.android.discovery

import dev.linuxlink.android.HostStore
import dev.linuxlink.android.pair.PairLink

/**
 * A computer announcing itself on this network (roadmap F6). Everything here
 * comes from an unauthenticated mDNS record: it fills in a form, it is never
 * trusted. Pairing still needs the PIN.
 */
data class FoundComputer(
    /** The mDNS instance name; the key while it is on the network. */
    val serviceName: String,
    /** What it calls itself, cleaned for display. */
    val name: String,
    /** Its device id, or blank when the record has none. */
    val id: String,
    /** The address it is listed at: the first of [candidates] that answered. */
    val address: String,
    val controlPort: Int,
    val streamPort: Int,
    /** Every announced address worth trying, best first (see [FoundComputers.rankAddresses]). */
    val candidates: List<String> = listOf(address),
) {
    fun toHost(): HostStore.Host = HostStore.Host(address, streamPort, controlPort)
}

object FoundComputers {
    /**
     * The announced addresses worth trying, best first: the phone found the
     * computer on this network, so private IPv4 (the LAN) comes first, then
     * any other IPv4 (Tailscale's 100.64/10 among them), then IPv6. A server
     * announces every address it has, Docker and VM bridges included, so the
     * caller tries them in turn; at most [MAX_CANDIDATES].
     */
    fun rankAddresses(addresses: List<String>): List<String> {
        val v4 = addresses.filter { isIpv4(it) }.distinct()
        val ranked = v4.filter { isPrivateIpv4(it) } +
            v4.filterNot { isPrivateIpv4(it) } +
            addresses.filter { it.contains(':') }.distinct()
        return ranked.take(MAX_CANDIDATES)
    }

    private const val MAX_CANDIDATES = 4

    /**
     * A found computer from its resolved record, or null without a usable
     * address. A missing or bad `stream` port falls back to the default.
     */
    fun fromRecord(
        serviceName: String,
        port: Int,
        txt: Map<String, String>,
        addresses: List<String>,
    ): FoundComputer? {
        val candidates = rankAddresses(addresses)
        val address = candidates.firstOrNull() ?: return null
        val control = port.takeIf { it in 1..65535 } ?: HostStore.DEFAULT_CONTROL_PORT
        val stream = txt["stream"]?.toIntOrNull()?.takeIf { it in 1..65535 }
            ?: HostStore.DEFAULT_STREAMING_PORT
        val name = PairLink.displayName(txt["name"]?.takeIf { it.isNotBlank() } ?: serviceName)
        return FoundComputer(
            serviceName = serviceName,
            name = name.ifBlank { address },
            id = txt["id"].orEmpty().trim(),
            address = address,
            controlPort = control,
            streamPort = stream,
            candidates = candidates,
        )
    }

    /** What Home shows: found computers not saved, and saved ones found. */
    data class View(
        val unsaved: List<FoundComputer>,
        /** Addresses of saved computers that are on this network now. */
        val savedOnline: Set<String>,
    )

    /**
     * A found computer is a saved one only when it answered at a saved
     * address. The record's device id is not used: mDNS is unauthenticated,
     * so anyone can announce any id, and a computer that moved must be
     * offered at its new address, not hidden behind its old card.
     */
    fun merge(found: List<FoundComputer>, saved: List<HostStore.Host>): View {
        val savedAddresses = saved.map { it.address }.toSet()
        val savedOnline = found.map { it.address }.filter { it in savedAddresses }.toSet()
        val unsaved = found.filter { it.address !in savedAddresses }.distinctBy { it.address }
        return View(unsaved, savedOnline)
    }

    private fun isIpv4(address: String): Boolean {
        val parts = address.split('.')
        return parts.size == 4 && parts.all { p -> p.toIntOrNull()?.let { it in 0..255 } == true }
    }

    private fun isPrivateIpv4(address: String): Boolean {
        val (a, b) = address.split('.').map { it.toInt() }
        return a == 10 || (a == 172 && b in 16..31) || (a == 192 && b == 168)
    }
}
