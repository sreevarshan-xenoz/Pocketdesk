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

    /** BB3: a computer that said the phone is not paired is not shown as paired. */
    @org.junit.Test
    fun `a computer that unpaired the phone says so`() {
        val host = dev.pocketdesk.android.HostStore.Host("192.0.2.1", dev.pocketdesk.android.HostStore.DEFAULT_STREAMING_PORT)
        val parts = HostStatus.parts(host, named = false, online = false, paired = true, auto = false, unpairedByIt = true)
        org.junit.Assert.assertTrue(HostStatus.Part.UnpairedByIt in parts)
        org.junit.Assert.assertFalse(HostStatus.Part.Paired in parts)
        org.junit.Assert.assertEquals(
            "not paired here: as before",
            HostStatus.Part.NotPaired,
            HostStatus.parts(host, named = false, online = false, paired = false, auto = false, unpairedByIt = true)[0],
        )
    }

    /** UX1: when it was last used comes last, and only once it has been. */
    @org.junit.Test
    fun `last used comes last once known`() {
        val host = dev.pocketdesk.android.HostStore.Host("192.0.2.1", dev.pocketdesk.android.HostStore.DEFAULT_STREAMING_PORT)
        val parts = HostStatus.parts(host, named = false, online = false, paired = true, auto = false, lastUsed = 1_000L)
        org.junit.Assert.assertEquals(HostStatus.Part.LastUsed(1_000L), parts.last())
        org.junit.Assert.assertTrue(
            HostStatus.parts(host, named = false, online = false, paired = true, auto = false)
                .none { it is HostStatus.Part.LastUsed },
        )
    }

    /** PR #575 review I1: the card's own buckets, said from the app's plurals. */
    @org.junit.Test
    fun `how long ago falls into minutes hours and days`() {
        val min = 60_000L
        val now = 100L * 24 * 60 * min
        org.junit.Assert.assertEquals(HostStatus.Ago.JustNow, HostStatus.ago(now - 59_999L, now))
        org.junit.Assert.assertEquals("ahead of now", HostStatus.Ago.JustNow, HostStatus.ago(now + 3 * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Minutes(1), HostStatus.ago(now - min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Minutes(59), HostStatus.ago(now - 59 * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Hours(1), HostStatus.ago(now - 60 * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Hours(23), HostStatus.ago(now - (24 * 60 - 1) * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Days(1), HostStatus.ago(now - 24 * 60 * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.Days(6), HostStatus.ago(now - (7 * 24 * 60 - 1) * min, now))
        org.junit.Assert.assertEquals(HostStatus.Ago.LongAgo, HostStatus.ago(now - 7 * 24 * 60 * min, now))
    }
}
