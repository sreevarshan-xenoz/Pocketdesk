package dev.pocketdesk.android.doctor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionChecksTest {
    private val home = PhoneState(link = Link.WIFI, lanPrefixes = listOf("192.168.1.0" to 24))

    private fun codes(
        state: PhoneState = home,
        address: String = "192.168.1.20",
        probes: Probes = Probes(Tcp.OK, identity = Identity.MATCHES, udp = Udp.ANSWERED),
        canWake: Boolean = false,
    ) = ConnectionChecks.run(state, address, probes, canWake).map { it.code }

    private fun finding(code: String, findings: List<Finding>) = findings.single { it.code == code }

    @Test
    fun `a healthy computer on the home network passes every check`() {
        val findings = ConnectionChecks.run(
            home, "192.168.1.20", Probes(Tcp.OK, identity = Identity.MATCHES, udp = Udp.ANSWERED), false,
        )
        assertEquals(listOf("on_wifi", "same_network", "answers", "key_matches", "video_port_ok"), findings.map { it.code })
        assertTrue(findings.all { it.status == Status.GOOD })
    }

    @Test
    fun `addresses are classified`() {
        assertEquals(AddressKind.PRIVATE_V4, ConnectionChecks.kindOf("192.168.1.20"))
        assertEquals(AddressKind.PRIVATE_V4, ConnectionChecks.kindOf("10.0.0.5"))
        assertEquals(AddressKind.PRIVATE_V4, ConnectionChecks.kindOf("172.20.1.1"))
        assertEquals(AddressKind.PUBLIC, ConnectionChecks.kindOf("172.32.1.1"))
        assertEquals(AddressKind.TAILNET, ConnectionChecks.kindOf("100.101.102.103"))
        assertEquals(AddressKind.TAILNET, ConnectionChecks.kindOf("desk.tail1234.ts.net"))
        assertEquals(AddressKind.LINK_LOCAL, ConnectionChecks.kindOf("169.254.3.4"))
        assertEquals(AddressKind.PUBLIC, ConnectionChecks.kindOf("203.0.113.9"))
        assertEquals(AddressKind.HOSTNAME, ConnectionChecks.kindOf("desk.local"))
        assertEquals(AddressKind.OTHER_V6, ConnectionChecks.kindOf("2001:db8::1"))
    }

    @Test
    fun `the same network is decided by the phone's prefixes`() {
        assertTrue(ConnectionChecks.onSameNetwork("192.168.1.20", listOf("192.168.1.0" to 24)))
        assertFalse(ConnectionChecks.onSameNetwork("192.168.2.20", listOf("192.168.1.0" to 24)))
        assertTrue(ConnectionChecks.onSameNetwork("10.1.200.3", listOf("10.1.0.0" to 16)))
        assertFalse(ConnectionChecks.onSameNetwork("desk.local", listOf("192.168.1.0" to 24)))
        val elsewhere = home.copy(lanPrefixes = listOf("10.0.0.0" to 24))
        assertTrue("other_network" in codes(state = elsewhere, probes = Probes(Tcp.TIMEOUT)))
        // Reached anyway (a routed subnet): nothing to warn about.
        assertTrue("reached_through_router" in codes(state = elsewhere))
    }

    @Test
    fun `each way the computer can fail to answer has its own finding`() {
        val p = { tcp: Tcp -> Probes(tcp) }
        assertTrue("refused" in codes(probes = p(Tcp.REFUSED)))
        assertTrue("no_route" in codes(probes = p(Tcp.NO_ROUTE)))
        assertTrue("timeout" in codes(probes = p(Tcp.TIMEOUT)))
        assertTrue("denied" in codes(probes = p(Tcp.DENIED)))
        assertTrue("no_name" in codes(probes = p(Tcp.NO_NAME)))
        assertTrue("connect_failed" in codes(probes = p(Tcp.OTHER)))
    }

    @Test
    fun `no route offers Wake only when the computer can be woken`() {
        val withWake = ConnectionChecks.run(home, "192.168.1.20", Probes(Tcp.NO_ROUTE), canWake = true)
        assertEquals(FixAction.WAKE, finding("no_route", withWake).action)
        val without = ConnectionChecks.run(home, "192.168.1.20", Probes(Tcp.NO_ROUTE), canWake = false)
        assertEquals(FixAction.NONE, finding("no_route", without).action)
    }

    @Test
    fun `a changed key offers only pairing again`() {
        val findings = ConnectionChecks.run(
            home, "192.168.1.20", Probes(Tcp.OK, identity = Identity.CHANGED, udp = Udp.ANSWERED), false,
        )
        val changed = finding("key_changed", findings)
        assertEquals(Status.BAD, changed.status)
        assertEquals(FixAction.PAIR_AGAIN, changed.action)
    }

    @Test
    fun `a computer that closed before saying who it is is not called something else`() {
        val findings = ConnectionChecks.run(home, "192.168.1.20", Probes(Tcp.OK, identity = Identity.CLOSED), false)
        assertEquals(Status.WARN, finding("closed_early", findings).status)
    }

    @Test
    fun `denied asks for the local network permission only when that is what is missing`() {
        val missing = ConnectionChecks.run(home.copy(localNetworkAllowed = false), "192.168.1.20", Probes(Tcp.DENIED), false)
        assertEquals(FixAction.ASK_LOCAL_NETWORK, finding("denied", missing).action)
        val other = ConnectionChecks.run(home, "192.168.1.20", Probes(Tcp.DENIED), false)
        assertEquals(FixAction.NONE, finding("denied", other).action)
    }

    @Test
    fun `a silent video port is a problem only when the computer answered`() {
        assertTrue("video_port_blocked" in codes(probes = Probes(Tcp.OK, udp = Udp.SILENT)))
        assertTrue("video_port_not_checked" in codes(probes = Probes(Tcp.TIMEOUT, udp = Udp.SILENT)))
        assertTrue("video_port_closed" in codes(probes = Probes(Tcp.OK, udp = Udp.REFUSED)))
    }

    @Test
    fun `a VPN that swallows home traffic is told apart from one that does not`() {
        val vpn = home.copy(vpn = true)
        assertTrue("vpn_captures_lan" in codes(state = vpn, probes = Probes(Tcp.TIMEOUT, tcpViaWifi = Tcp.OK)))
        assertTrue("vpn_on" in codes(state = vpn, probes = Probes(Tcp.OK)))
        assertFalse(codes().any { it.startsWith("vpn") })
    }

    @Test
    fun `a Tailscale address needs Tailscale on`() {
        assertTrue("tailscale_off" in codes(address = "100.101.102.103"))
        assertTrue(
            "tailscale_on" in codes(state = home.copy(vpn = true, vpnTailnet = true), address = "100.101.102.103"),
        )
        // Another VPN is on, so Tailscale is not.
        assertTrue("vpn_not_tailnet" in codes(state = home.copy(vpn = true), address = "100.101.102.103"))
    }

    @Test
    fun `the phone's own network comes first`() {
        assertEquals("offline", codes(state = home.copy(link = Link.NONE)).first())
        assertEquals("captive_portal", codes(state = home.copy(captivePortal = true)).first())
        assertEquals("ipv6_only", codes(state = home.copy(hasIpv4 = false)).first())
        assertEquals(
            "on_mobile_home_address",
            codes(state = home.copy(link = Link.CELLULAR, lanPrefixes = emptyList()), probes = Probes(Tcp.TIMEOUT))[1],
        )
    }

    @Test
    fun `local network access is shown only where Android asks for it`() {
        assertFalse(codes().any { it.startsWith("local_network") })
        assertTrue("local_network_denied" in codes(state = home.copy(localNetworkAllowed = false)))
    }

    @Test
    fun `background checks appear only when a background feature is on`() {
        val optimized = home.copy(batteryOptimized = true, dataSaver = true)
        assertFalse(codes(state = optimized).any { it == "battery_optimized" || it == "data_saver" })
        val on = optimized.copy(backgroundFeaturesOn = true)
        assertTrue("battery_optimized" in codes(state = on))
        assertTrue("data_saver" in codes(state = on))
        assertTrue(
            "background_restricted" in codes(state = on.copy(backgroundRestricted = true)),
        )
    }

    @Test
    fun `the copied report carries no address, MAC or id`() {
        val findings = ConnectionChecks.run(
            home.copy(vpn = true, backgroundFeaturesOn = true, batteryOptimized = true),
            "192.168.1.20",
            Probes(Tcp.TIMEOUT, tcpViaWifi = Tcp.OK, identity = Identity.CHANGED, udp = Udp.SILENT),
            canWake = true,
        )
        val report = DoctorReport.text(findings, android = "16", manufacturer = "samsung", app = "0.2.0")
        assertFalse(Regex("""\b\d{1,3}(\.\d{1,3}){3}\b""").containsMatchIn(report))
        assertFalse(Regex("""([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}""").containsMatchIn(report))
        assertFalse(Regex("""[0-9a-fA-F]{16,}""").containsMatchIn(report))
        assertTrue(report.contains("bad: vpn_captures_lan"))
    }
}

class VersionProbeTest {
    @Test
    fun `the UDP probe is a full-size long-header packet with a reserved version`() {
        val packet = ConnectionProbe.versionProbe()
        assertEquals(1200, packet.size)
        assertEquals(0xC0, packet[0].toInt() and 0xC0)
        // Reserved versions have 0xa in the low nibble of every byte.
        for (i in 1..4) assertEquals(0x0A, packet[i].toInt() and 0x0F)
        assertEquals(8, packet[5].toInt())
        assertEquals(8, packet[14].toInt())
    }
}

class VersionNegotiationTest {
    @Test
    fun `only a Version Negotiation reply counts as the video port answering`() {
        val vn = ByteArray(40).also { it[0] = 0x80.toByte() }
        assertTrue(ConnectionProbe.isVersionNegotiation(vn, vn.size))
        val other = ByteArray(40).also { it[0] = 0x80.toByte(); it[1] = 1 }
        assertFalse(ConnectionProbe.isVersionNegotiation(other, other.size))
        val short = ByteArray(40).also { it[0] = 0x40 }
        assertFalse(ConnectionProbe.isVersionNegotiation(short, short.size))
    }
}
