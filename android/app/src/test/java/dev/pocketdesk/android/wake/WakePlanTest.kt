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
    ) = WakePlan.plan(settings, learned, wan)

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
    fun outsidePortsMustBeRealPorts() {
        assertTrue(WakePlan.validPort(""))
        assertTrue(WakePlan.validPort("9"))
        assertTrue(WakePlan.validPort("65535"))
        assertFalse(WakePlan.validPort("0"))
        assertFalse(WakePlan.validPort("70000"))
        assertFalse(WakePlan.validPort("nine"))
    }
}
