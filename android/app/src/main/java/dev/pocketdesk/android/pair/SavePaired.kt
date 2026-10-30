package dev.pocketdesk.android.pair

import android.content.Context
import dev.pocketdesk.android.HostStore

/**
 * Save the computer a pairing [link] paired with at [address]: its card,
 * the id it answered with, the name the link carried (U3, until a
 * connection says it again) and its internet road when the link names one.
 */
fun savePaired(context: Context, link: PairLink, address: String, serverId: String) {
    HostStore.save(context, HostStore.Host(address, link.streamPort, link.controlPort))
    HostStore.savePairedDesktop(context, address, serverId)
    HostStore.saveClaimedName(context, address, link.name)
    link.endpoint?.let { HostStore.saveWanIdentity(context, address, it) }
}

/** The ids of the computers this phone is paired with, by their cards. */
fun pairedComputerIds(context: Context): Set<String> =
    HostStore.hosts(context).mapNotNull { HostStore.pairedDesktopId(context, it.address) }.toSet()
