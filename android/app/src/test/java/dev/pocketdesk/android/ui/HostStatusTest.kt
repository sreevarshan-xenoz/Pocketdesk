package dev.pocketdesk.android.ui

import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.ui.HostStatus.Part
import org.junit.Assert.assertEquals
import org.junit.Test

class HostStatusTest {
    private val usual = HostStore.Host("192.0.2.40", HostStore.DEFAULT_STREAMING_PORT, HostStore.DEFAULT_CONTROL_PORT)

    @Test
    fun aNamedComputerOnThisNetworkShowsItsAddressAndStanding() {
        assertEquals(
            listOf(Part.Address("192.0.2.40"), Part.OnThisNetwork, Part.Paired),
            HostStatus.parts(usual, named = true, online = true, paired = true, auto = false),
        )
    }

    @Test
    fun anUnnamedOneIsTitledByItsAddressSoTheLineLeavesItOut() {
        assertEquals(
            listOf(Part.NotPaired, Part.OpensAtLaunch),
            HostStatus.parts(usual, named = false, online = false, paired = false, auto = true),
        )
    }

    @Test
    fun portsShowOnlyWhenTheyAreNotTheUsualOnes() {
        val moved = usual.copy(port = 4817)
        assertEquals(
            listOf(Part.Paired, Part.Ports(4817, HostStore.DEFAULT_CONTROL_PORT)),
            HostStatus.parts(moved, named = false, online = false, paired = true, auto = false),
        )
    }
}
