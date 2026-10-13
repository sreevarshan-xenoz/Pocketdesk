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
    val address: String,
    val controlPort: Int,
    val streamPort: Int,
) {
    fun toHost(): HostStore.Host = HostStore.Host(address, streamPort, controlPort)
}

object FoundComputers {
    /**
     * The address to dial: the phone found it on this network, so a private
     * IPv4 address (the LAN) comes first, then any other IPv4 (Tailscale's
     * 100.64/10 among them), then IPv6. Null when there is none.
     */
    fun pickAddress(addresses: List<String>): String? {
        val v4 = addresses.filter { isIpv4(it) }
        return v4.firstOrNull { isPrivateIpv4(it) }
            ?: v4.firstOrNull()
            ?: addresses.firstOrNull { it.contains(':') }
    }

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
        val address = pickAddress(addresses) ?: return null
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
        )
    }

    /** What Home shows: found computers not saved, and saved ones found. */
    data class View(
        val unsaved: List<FoundComputer>,
        /** Addresses of saved computers that are on this network now. */
        val savedOnline: Set<String>,
    )

    /**
     * A found computer is a saved one when its address is saved, or when its
     * device id is the one a saved address paired with (its address changed).
     */
    fun merge(
        found: List<FoundComputer>,
        saved: List<HostStore.Host>,
        pairedId: (address: String) -> String?,
    ): View {
        val savedOnline = mutableSetOf<String>()
        val unsaved = found.filter { computer ->
            val match = saved.firstOrNull { host ->
                host.address == computer.address ||
                    (computer.id.isNotEmpty() && pairedId(host.address) == computer.id)
            }
            if (match != null) savedOnline += match.address
            match == null
        }.distinctBy { it.address }
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
