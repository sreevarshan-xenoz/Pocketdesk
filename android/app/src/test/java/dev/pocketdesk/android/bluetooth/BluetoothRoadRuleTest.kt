package dev.pocketdesk.android.bluetooth

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BluetoothRoadRuleTest {
    private val computer = "00:00:5E:00:53:01"
    private val earbuds = "00:00:5E:00:53:02"

    private fun dial(
        enabled: Boolean = true,
        permitted: Boolean = true,
        radioOn: Boolean = true,
        knownAddress: String? = computer,
        bonded: Set<String> = setOf(computer, earbuds),
        hasUsableNetwork: Boolean = true,
        otherRoadsFailed: Boolean = true,
        now: Long = 100_000L,
        restUntil: Long = 0L,
    ) = BluetoothRoadRule.shouldDial(enabled, permitted, radioOn, knownAddress, bonded, hasUsableNetwork, otherRoadsFailed, now, restUntil)

    @Test
    fun only_dials_a_known_computer_address() {
        assertTrue(dial())
        // Bonded devices that no computer named (earbuds, a car) are never dialed.
        assertFalse(dial(bonded = setOf(earbuds)))
        assertFalse(dial(knownAddress = null))
        assertFalse(dial(bonded = emptySet()))
        // The case of the letters is not a difference.
        assertTrue(dial(knownAddress = computer.lowercase()))
    }

    @Test
    fun waits_for_other_roads() {
        assertFalse(dial(otherRoadsFailed = false, hasUsableNetwork = true))
        assertTrue(dial(otherRoadsFailed = false, hasUsableNetwork = false))
        assertTrue(dial(otherRoadsFailed = true, hasUsableNetwork = true))
    }

    @Test
    fun backs_off_and_doubles() {
        assertEquals(30_000L, BluetoothRoadRule.nextRest(1))
        assertEquals(60_000L, BluetoothRoadRule.nextRest(2))
        assertEquals(480_000L, BluetoothRoadRule.nextRest(5))
        assertEquals(480_000L, BluetoothRoadRule.nextRest(9))
        assertEquals(480_000L, BluetoothRoadRule.nextRest(100))
        assertEquals(0L, BluetoothRoadRule.nextRest(0))
        assertFalse(dial(now = 1_000L, restUntil = 2_000L))
        assertTrue(dial(now = 2_000L, restUntil = 2_000L))
    }

    @Test
    fun off_without_permission() {
        assertFalse(dial(permitted = false))
        assertFalse(dial(enabled = false))
        assertFalse(dial(radioOn = false))
    }

    @Test
    fun prefers_wifi_when_it_returns() {
        assertTrue(BluetoothRoadRule.keepBluetooth(BluetoothRoadRule.ROAD_BLUETOOTH, homeOrInternetUp = false))
        assertFalse(BluetoothRoadRule.keepBluetooth(BluetoothRoadRule.ROAD_BLUETOOTH, homeOrInternetUp = true))
        // No Bluetooth link to keep on another road.
        assertFalse(BluetoothRoadRule.keepBluetooth(0, homeOrInternetUp = false))
        assertFalse(BluetoothRoadRule.keepBluetooth(1, homeOrInternetUp = false))
    }

    @Test
    fun probes_home_on_a_tick_only_with_a_local_network() {
        // The periodic tick: only with Wi-Fi, Ethernet or the hotspot up.
        assertTrue(BluetoothRoadRule.probeHome(networkChanged = false, usableNetwork = true, localNetwork = true))
        assertFalse(BluetoothRoadRule.probeHome(networkChanged = false, usableNetwork = true, localNetwork = false))
        // Once on each change to a usable network, mobile data included.
        assertTrue(BluetoothRoadRule.probeHome(networkChanged = true, usableNetwork = true, localNetwork = false))
        assertFalse(BluetoothRoadRule.probeHome(networkChanged = true, usableNetwork = false, localNetwork = false))
    }

    @Test
    fun leaves_for_home_or_once_for_the_internet_road() {
        assertTrue(BluetoothRoadRule.leaveBluetooth(homeUp = true, networkChanged = false, internetRoad = false))
        assertTrue(BluetoothRoadRule.leaveBluetooth(homeUp = false, networkChanged = true, internetRoad = true))
        // Not on the tick, and not for a computer with no internet road.
        assertFalse(BluetoothRoadRule.leaveBluetooth(homeUp = false, networkChanged = false, internetRoad = true))
        assertFalse(BluetoothRoadRule.leaveBluetooth(homeUp = false, networkChanged = true, internetRoad = false))
    }

    @Test
    fun a_refusal_no_retry_changes_is_told() {
        assertTrue(
            BluetoothRoadRule.finalRefusal(
                "Error(PocketdeskErrorDto { code: 3, message: \"This phone has not verified the computer at 192.0.2.10 yet: connect at home first\", is_retryable: false })",
            ),
        )
        assertFalse(BluetoothRoadRule.finalRefusal("Error(PocketdeskErrorDto { code: 1, message: \"timed out\", is_retryable: true })"))
        // Another failure no retry changes is not this refusal.
        assertFalse(
            BluetoothRoadRule.finalRefusal(
                "Error(PocketdeskErrorDto { code: 3, message: \"connection to 192.0.2.10:1716 failed: TLS handshake: certificate changed\", is_retryable: false })",
            ),
        )
        assertFalse(BluetoothRoadRule.finalRefusal("Connected"))
        assertFalse(BluetoothRoadRule.finalRefusal(null))
    }

    @Test
    fun what_the_computer_said_reads_or_is_nothing() {
        val said = BluetoothRoadRule.info(
            """{"address":"00:00:5E:00:53:01","service":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13","link":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12"}""",
        )
        assertEquals(computer, said?.address)
        assertEquals("b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13", said?.service.toString())
        assertEquals("b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12", said?.link.toString())
        assertNull(BluetoothRoadRule.info(""))
        assertNull(BluetoothRoadRule.info(null))
        assertNull(BluetoothRoadRule.info("not json"))
        assertNull(BluetoothRoadRule.info("""{"address":"00:00:5E:00:53","service":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13","link":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12"}"""))
        assertNull(BluetoothRoadRule.info("""{"address":"00:00:5E:00:53:01","service":"nope","link":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12"}"""))
        assertNull(BluetoothRoadRule.info("""{"address":"00:00:5E:00:53:01","service":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13"}"""))
    }

    @Test
    fun a_link_waits_while_one_is_live_and_tries_a_closing_channel_again() {
        // One transfer link at a time: none taken while one is up.
        assertFalse(BluetoothRoadRule.takeLinkRequest(linkLive = true))
        assertTrue(BluetoothRoadRule.takeLinkRequest(linkLive = false))
        // The last link's channel may still be closing: tried again for about 6 s,
        // so the poll, the probe and the tries stay within the computer's 10 s.
        assertTrue(BluetoothRoadRule.tryLinkAgain(startedAt = 0L, now = 5_499L))
        assertFalse(BluetoothRoadRule.tryLinkAgain(startedAt = 0L, now = 5_500L))
        assertTrue(BluetoothRoadRule.LINK_TRY_MS <= 6_000L)
    }
}

class BluetoothTransferRuleTest {
    @Test
    fun big_files_wait_for_wifi() {
        assertEquals(TransferVerdict.WaitForWifi, BluetoothRoadRule.transferMayStart(road = 2, bytes = 8_388_609L, audioBusy = false))
        // Exactly 8 MiB goes.
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 2, bytes = 8_388_608L, audioBusy = false))
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 2, bytes = 1L, audioBusy = false))
        // Home and the internet: any size.
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 0, bytes = 8_388_609L, audioBusy = false))
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 0, bytes = Long.MAX_VALUE, audioBusy = false))
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 1, bytes = 8_388_609L, audioBusy = false))
        // Too big waits for Wi-Fi whatever the audio does: the call ending does not let it go.
        assertEquals(TransferVerdict.WaitForWifi, BluetoothRoadRule.transferMayStart(road = 2, bytes = 8_388_609L, audioBusy = true))
    }

    @Test
    fun transfers_wait_for_audio() {
        assertEquals(TransferVerdict.WaitForAudio, BluetoothRoadRule.transferMayStart(road = 2, bytes = 1_000L, audioBusy = true))
        assertEquals(TransferVerdict.WaitForAudio, BluetoothRoadRule.transferMayStart(road = 2, bytes = 8_388_608L, audioBusy = true))
        // Off Bluetooth the radio is the headset's alone: transfers go.
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 0, bytes = 1_000L, audioBusy = true))
        assertEquals(TransferVerdict.Go, BluetoothRoadRule.transferMayStart(road = 1, bytes = 1_000L, audioBusy = true))
    }

    @Test
    fun off_without_permission() {
        // The switch says why it does nothing...
        assertEquals(BluetoothRoadRule.SwitchSays.NEEDS_PERMISSION, BluetoothRoadRule.switchSays(on = true, permitted = false))
        assertEquals(BluetoothRoadRule.SwitchSays.WORKS, BluetoothRoadRule.switchSays(on = true, permitted = true))
        assertEquals(BluetoothRoadRule.SwitchSays.OFF, BluetoothRoadRule.switchSays(on = false, permitted = false))
        assertEquals(BluetoothRoadRule.SwitchSays.OFF, BluetoothRoadRule.switchSays(on = false, permitted = true))
        // ...and the road is never dialed without the permission.
        assertFalse(
            BluetoothRoadRule.shouldDial(
                enabled = true,
                permitted = false,
                radioOn = true,
                knownAddress = "00:00:5E:00:53:01",
                bonded = setOf("00:00:5E:00:53:01"),
                hasUsableNetwork = false,
                otherRoadsFailed = true,
                now = 100_000L,
                restUntil = 0L,
            ),
        )
    }
}
