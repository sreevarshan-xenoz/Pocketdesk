package dev.pocketdesk.android.wake

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WakePlanTest {
    private val info = WakeInfo(mac = "02:11:22:33:44:55", broadcast = "192.168.1.255", wired = true, port = 40009)
    private val identity = listOf("192.168.1.20:54762", "100.70.1.2:1", "203.0.113.7:54762", "[2001:db8::1]:5")

    private fun plan(
        settings: WakeSettings = WakeSettings(),
        learned: WakeInfo? = info,
        wan: List<String> = identity,
        atHome: Boolean = true,
    ) = WakePlan.plan(settings, learned, wan, atHome)

    @Test
    fun autoWakesOnThisNetworkAndFromOutside() {
        assertEquals(
            WakePlan.Plan.Direct(
                "02:11:22:33:44:55",
                listOf("192.168.1.255:9", "255.255.255.255:9", "203.0.113.7:40009"),
            ),
            plan(),
        )
    }

    @Test
    fun thisNetworkOnlySendsNoOutsidePacket() {
        assertEquals(
            WakePlan.Plan.Direct("02:11:22:33:44:55", listOf("192.168.1.255:9", "255.255.255.255:9")),
            plan(WakeSettings(method = WakeMethod.LAN)),
        )
    }

    @Test
    fun noOutsidePortMeansNoOutsidePacket() {
        val direct = plan(learned = info.copy(port = 0)) as WakePlan.Plan.Direct
        assertEquals(listOf("192.168.1.255:9", "255.255.255.255:9"), direct.targets)
    }

    @Test
    fun theUsersOutsideAddressAndPortWin() {
        val direct =
            plan(WakeSettings(outsideHost = "home.example.net", outsidePort = 7)) as WakePlan.Plan.Direct
        assertEquals("home.example.net:7", direct.targets.last())
        val portOnly = plan(WakeSettings(outsidePort = 7)) as WakePlan.Plan.Direct
        assertEquals("203.0.113.7:7", portOnly.targets.last())
    }

    @Test
    fun aTypedMacWinsAndWorksWithoutLearnedFacts() {
        val direct = plan(WakeSettings(mac = "aa-bb-cc-dd-ee-ff"), learned = null, wan = emptyList()) as WakePlan.Plan.Direct
        assertEquals("aa-bb-cc-dd-ee-ff", direct.mac)
        assertEquals(listOf("255.255.255.255:9"), direct.targets)
    }

    @Test
    fun noMacMeansNoWake() {
        assertEquals(WakePlan.Plan.NoMac, plan(learned = null))
        assertEquals(WakePlan.Plan.NoMac, plan(WakeSettings(mac = "not a mac"), learned = null))
    }

    @Test
    fun offAndRelayAreTheUsersChoice() {
        assertEquals(WakePlan.Plan.Off, plan(WakeSettings(method = WakeMethod.OFF)))
        assertEquals(
            WakePlan.Plan.Relay("192.168.1.5", "02:11:22:33:44:55", "192.168.1.255"),
            plan(WakeSettings(method = WakeMethod.RELAY, relay = "192.168.1.5")),
        )
        assertEquals(WakePlan.Plan.NoRelay, plan(WakeSettings(method = WakeMethod.RELAY)))
    }

    @Test
    fun thePublicAddressSkipsPrivateTailnetAndIpv6() {
        assertEquals("203.0.113.7", WakePlan.publicIpv4(identity))
        assertNull(WakePlan.publicIpv4(listOf("10.0.0.2:1", "172.20.1.1:1", "100.64.0.1:1", "169.254.3.3:1")))
        assertNull(WakePlan.publicIpv4(listOf("garbage", "")))
    }

    @Test
    fun atHomeIsSharingTheComputersBroadcast() {
        assertTrue(WakePlan.atHome(info, setOf("192.168.1.255")))
        assertFalse(WakePlan.atHome(info, setOf("10.0.255.255")))
        assertFalse(WakePlan.atHome(null, setOf("192.168.1.255")))
    }

    @Test
    fun awayFromHomeTheMacIsNotBroadcastOnSomeoneElsesNetwork() {
        assertEquals(
            WakePlan.Plan.Direct("02:11:22:33:44:55", listOf("203.0.113.7:40009")),
            plan(atHome = false),
        )
        assertEquals(WakePlan.Plan.Away, plan(WakeSettings(method = WakeMethod.LAN), atHome = false))
        assertEquals(WakePlan.Plan.Away, plan(learned = info.copy(port = 0), atHome = false))
    }

    @Test
    fun aTypedMacWithNothingLearnedStillBroadcasts() {
        val direct =
            plan(WakeSettings(mac = "AA:BB:CC:DD:EE:FF"), learned = null, wan = emptyList(), atHome = false)
                as WakePlan.Plan.Direct
        assertEquals(listOf("255.255.255.255:9"), direct.targets)
    }

    @Test
    fun learnedTargetsMustBeAddressesNotNames() {
        val direct = plan(learned = info.copy(broadcast = "evil.example.com")) as WakePlan.Plan.Direct
        assertEquals(listOf("255.255.255.255:9", "203.0.113.7:40009"), direct.targets)
    }

    @Test
    fun theOutsideAddressIsAHostWithoutAPort() {
        assertTrue(WakePlan.validHost(""))
        assertTrue(WakePlan.validHost("home.example.net"))
        assertTrue(WakePlan.validHost("203.0.113.7"))
        assertTrue(WakePlan.validHost("2001:db8::1"))
        assertFalse(WakePlan.validHost("home.example.net:5000"))
        assertFalse(WakePlan.validHost("two words"))
        val v6 = plan(WakeSettings(outsideHost = "2001:db8::1")) as WakePlan.Plan.Direct
        assertEquals("[2001:db8::1]:40009", v6.targets.last())
    }

    @Test
    fun outsidePortsMustBeRealPorts() {
        assertTrue(WakePlan.validPort(""))
        assertTrue(WakePlan.validPort("9"))
        assertTrue(WakePlan.validPort("65535"))
        assertFalse(WakePlan.validPort("0"))
        assertFalse(WakePlan.validPort("70000"))
        assertFalse(WakePlan.validPort("nine"))
    }

    @Test
    fun `a computer moved takes along the wakes sent through it`() {
        // AX2: a card waking through the computer at 192.0.2.1 kept that
        // address after the computer moved to 192.0.2.9, and its Wake failed.
        val through = WakeSettings(method = WakeMethod.RELAY, relay = "192.0.2.1", mac = "02:11:22:33:44:55")
        assertEquals(through.copy(relay = "192.0.2.9"), WakePlan.relayMoved(through, "192.0.2.1", "192.0.2.9"))
        assertNull(WakePlan.relayMoved(through, "192.0.2.5", "192.0.2.9"))
        // Kept for later while another method is on, it moves too (review);
        // one naming no computer, or another, is left alone.
        assertEquals(
            WakeSettings(relay = "192.0.2.9"),
            WakePlan.relayMoved(WakeSettings(relay = "192.0.2.1"), "192.0.2.1", "192.0.2.9"),
        )
        assertNull(WakePlan.relayMoved(WakeSettings(), "192.0.2.1", "192.0.2.9"))
    }

    @Test
    fun `a computer removed sends no other card's wake`() {
        // BC2: a card waking through the computer at 192.0.2.1 kept it after
        // it was removed, so Wake showed and failed, and a computer saved
        // later at that address would have been asked to send the wake.
        val through = WakeSettings(method = WakeMethod.RELAY, relay = "192.0.2.1", mac = "02:11:22:33:44:55")
        val after = WakePlan.relayRemoved(through, "192.0.2.1")
        assertEquals(through.copy(relay = ""), after)
        assertEquals(WakePlan.Plan.NoRelay, WakePlan.plan(after!!, null, emptyList(), atHome = true))
        assertNull(WakePlan.relayRemoved(through, "192.0.2.5"))
        assertNull(WakePlan.relayRemoved(WakeSettings(), "192.0.2.1"))
    }

    @Test
    fun `a relay removed is not shown as chosen`() {
        // BC2 review: the settings showed the first saved computer as the
        // one sending the wake, and Save chose it.
        val saved = listOf("192.0.2.2", "192.0.2.3")
        assertEquals("", WakePlan.relayShown(WakeSettings(method = WakeMethod.RELAY), saved))
        assertEquals("192.0.2.2", WakePlan.relayShown(WakeSettings(), saved))
        assertEquals("192.0.2.3", WakePlan.relayShown(WakeSettings(method = WakeMethod.RELAY, relay = "192.0.2.3"), saved))
    }
}
