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

        /** Paired here, but the computer said it no longer is (BB3). */
        data object UnpairedByIt : Part
        data object OpensAtLaunch : Part
        data class Ports(val stream: Int, val control: Int) : Part

        /** UX1: when it was last used, in ms since the epoch. */
        data class LastUsed(val at: Long) : Part
    }

    fun parts(
        host: HostStore.Host,
        named: Boolean,
        online: Boolean,
        paired: Boolean,
        auto: Boolean,
        unpairedByIt: Boolean = false,
        lastUsed: Long? = null,
    ): List<Part> = buildList {
        if (named) add(Part.Address(host.address))
        if (online) add(Part.OnThisNetwork)
        add(
            when {
                !paired -> Part.NotPaired
                // BB3: the card said "Paired" though the computer had unpaired the phone.
                unpairedByIt -> Part.UnpairedByIt
                else -> Part.Paired
            },
        )
        if (auto) add(Part.OpensAtLaunch)
        if (host.port != HostStore.DEFAULT_STREAMING_PORT || host.controlPort != HostStore.DEFAULT_CONTROL_PORT) {
            add(Part.Ports(host.port, host.controlPort))
        }
        if (lastUsed != null) add(Part.LastUsed(lastUsed))
    }

    /**
     * How long ago [at] was, as the card says it (PR #575 review I1: from
     * the app's own plurals, so its language holds; Android's relative
     * time follows the system's). A time ahead of [now] is just now.
     */
    sealed interface Ago {
        data object JustNow : Ago
        data class Minutes(val n: Int) : Ago
        data class Hours(val n: Int) : Ago
        data class Days(val n: Int) : Ago
        data object LongAgo : Ago
    }

    fun ago(at: Long, now: Long): Ago {
        val minutes = (now - at) / 60_000L
        return when {
            minutes < 1 -> Ago.JustNow
            minutes < 60 -> Ago.Minutes(minutes.toInt())
            minutes < 24 * 60 -> Ago.Hours((minutes / 60).toInt())
            minutes < 7 * 24 * 60 -> Ago.Days((minutes / (24 * 60)).toInt())
            else -> Ago.LongAgo
        }
    }
}
