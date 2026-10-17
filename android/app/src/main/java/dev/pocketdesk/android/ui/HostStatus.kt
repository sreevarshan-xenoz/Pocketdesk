package dev.pocketdesk.android.ui

import dev.pocketdesk.android.HostStore

/**
 * What a saved computer's card says under its name (U4), instead of its
 * ports: its address when the name is not the address, whether it is on
 * this network, whether this phone is paired with it, whether it opens at
 * launch, and its ports only when they are not the usual ones.
 */
object HostStatus {
    sealed interface Part {
        data class Address(val address: String) : Part
        data object OnThisNetwork : Part
        data object Paired : Part
        data object NotPaired : Part
        data object OpensAtLaunch : Part
        data class Ports(val stream: Int, val control: Int) : Part
    }

    fun parts(
        host: HostStore.Host,
        named: Boolean,
        online: Boolean,
        paired: Boolean,
        auto: Boolean,
    ): List<Part> = buildList {
        if (named) add(Part.Address(host.address))
        if (online) add(Part.OnThisNetwork)
        add(if (paired) Part.Paired else Part.NotPaired)
        if (auto) add(Part.OpensAtLaunch)
        if (host.port != HostStore.DEFAULT_STREAMING_PORT || host.controlPort != HostStore.DEFAULT_CONTROL_PORT) {
            add(Part.Ports(host.port, host.controlPort))
        }
    }
}
