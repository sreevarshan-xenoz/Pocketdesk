package dev.pocketdesk.android.status

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BatteryRuleTest {
    private val r54 = BatteryRule.Report(charge = 54, charging = false, low = false)

    @Test
    fun theChargeIsAWholePercentOfTheScaleAndABadReadingIsNone() {
        assertEquals(54, BatteryRule.report(level = 54, scale = 100, plugged = 0, low = false)?.charge)
        assertEquals(50, BatteryRule.report(level = 1, scale = 2, plugged = 0, low = false)?.charge)
        assertEquals(true, BatteryRule.report(level = 9, scale = 100, plugged = 2, low = true)?.charging)
        assertNull(BatteryRule.report(level = -1, scale = 100, plugged = 0, low = false))
        assertNull(BatteryRule.report(level = 5, scale = 0, plugged = 0, low = false))
        assertNull(BatteryRule.report(level = 101, scale = 100, plugged = 0, low = false))
    }

    @Test
    fun itIsSentAtTheStartOnAChangeAndEveryFiveMinutesOtherwise() {
        assertTrue("the first", BatteryRule.shouldSend(null, 0, r54, 0))
        assertFalse("the same, soon", BatteryRule.shouldSend(r54, 0, r54, 60_000))
        assertTrue("a percent", BatteryRule.shouldSend(r54, 0, r54.copy(charge = 53), 1_000))
        assertTrue("the plug", BatteryRule.shouldSend(r54, 0, r54.copy(charging = true), 1_000))
        assertTrue("low", BatteryRule.shouldSend(r54, 0, r54.copy(low = true), 1_000))
        assertTrue("so its age stays fresh", BatteryRule.shouldSend(r54, 0, r54, BatteryRule.RESEND_MS))
        // L9: the state's network and ringer at once, its bars and storage with the resend.
        val withState = r54.copy(state = PhoneStateRule.State(network = PhoneStateRule.WIFI, signal = 3))
        assertFalse("the bars", BatteryRule.shouldSend(withState, 0, withState.copy(state = withState.state.copy(signal = 1)), 1_000))
        assertTrue("the network", BatteryRule.shouldSend(withState, 0, withState.copy(state = withState.state.copy(network = PhoneStateRule.MOBILE)), 1_000))
        assertTrue("the bars, in time", BatteryRule.shouldSend(withState, 0, withState.copy(state = withState.state.copy(signal = 1)), BatteryRule.RESEND_MS))
    }

    @Test
    fun batteryManagersReadingIsTakenOverAStaleBroadcast() {
        val stale = BatteryRule.Report(charge = 72, charging = true, low = false)
        assertEquals(79, BatteryRule.fresh(stale, 79, BatteryRule.STATUS_CHARGING)?.charge)
        assertEquals("full is still on the charger", true, BatteryRule.fresh(stale, 100, BatteryRule.STATUS_FULL)?.charging)
        assertEquals("unplugged", false, BatteryRule.fresh(stale, 70, BatteryRule.STATUS_DISCHARGING)?.charging)
        assertEquals("unknown capacity keeps the broadcast's", 72, BatteryRule.fresh(stale, Int.MIN_VALUE, 1)?.charge)
        assertEquals("unknown status keeps the broadcast's", true, BatteryRule.fresh(stale, 50, 1)?.charging)
        assertEquals("no broadcast, the capacity alone", 40, BatteryRule.fresh(null, 40, BatteryRule.STATUS_DISCHARGING)?.charge)
        assertNull(BatteryRule.fresh(null, -1, 1))
    }

    @Test
    fun theBodyIsKdeConnectsBatteryPacket() {
        assertEquals(
            """{"currentCharge":54,"isCharging":false,"thresholdEvent":0}""",
            BatteryRule.body(r54),
        )
        assertEquals(
            """{"currentCharge":9,"isCharging":true,"thresholdEvent":1}""",
            BatteryRule.body(BatteryRule.Report(charge = 9, charging = true, low = true)),
        )
        assertEquals(
            """{"currentCharge":54,"isCharging":false,"thresholdEvent":0,"ringer":"vibrate"}""",
            BatteryRule.body(r54.copy(state = PhoneStateRule.State(ringer = PhoneStateRule.VIBRATE))),
        )
    }
}
