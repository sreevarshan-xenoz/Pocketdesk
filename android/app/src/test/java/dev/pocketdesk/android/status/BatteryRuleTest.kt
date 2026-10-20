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
