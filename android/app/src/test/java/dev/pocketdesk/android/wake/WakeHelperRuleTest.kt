package dev.pocketdesk.android.wake

import dev.pocketdesk.android.wake.WakeHelperRule.State
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WakeHelperRuleTest {
    @Test
    fun `it serves only with the switch on, on a charger and on wifi`() {
        assertEquals(State.SERVING, WakeHelperRule.state(on = true, charging = true, wifi = true))
        assertEquals(State.WAITING, WakeHelperRule.state(on = true, charging = false, wifi = true))
        assertEquals(State.WAITING, WakeHelperRule.state(on = true, charging = true, wifi = false))
        assertEquals(State.WAITING, WakeHelperRule.state(on = true, charging = false, wifi = false))
        for (charging in listOf(true, false)) {
            for (wifi in listOf(true, false)) {
                assertEquals(State.OFF, WakeHelperRule.state(on = false, charging = charging, wifi = wifi))
            }
        }
    }

    @Test
    fun `the service is wanted exactly when the switch is on`() {
        assertTrue(WakeHelperRule.wanted(true))
        assertFalse(WakeHelperRule.wanted(false))
    }

    @Test
    fun `it tells its computer about itself first, when it changes, and now and then`() {
        val key = WakeHelperRule.offerKey("aa", listOf("https://r2/", "https://r1/"))
        assertEquals("aa|https://r1/,https://r2/", key)
        assertTrue(WakeHelperRule.offerDue(1_000, null, null, key))
        assertFalse(WakeHelperRule.offerDue(2_000, 1_000, key, key))
        assertTrue("changed", WakeHelperRule.offerDue(2_000, 1_000, "other", key))
        assertFalse(WakeHelperRule.offerDue(1_000 + WakeHelperRule.OFFER_EVERY_MS - 1, 1_000, key, key))
        assertTrue(WakeHelperRule.offerDue(1_000 + WakeHelperRule.OFFER_EVERY_MS, 1_000, key, key))
        assertTrue("the clock went back", WakeHelperRule.offerDue(500, 1_000, key, key))
    }

    @Test
    fun `a duty or an info is kept only from the computer this phone paired that address with`() {
        assertTrue(WakeHelperRule.keep("computer-1", "computer-1"))
        assertFalse("another computer's id", WakeHelperRule.keep("computer-2", "computer-1"))
        assertFalse("not paired", WakeHelperRule.keep("computer-1", null))
        assertFalse(WakeHelperRule.keep(null, "computer-1"))
        assertFalse(WakeHelperRule.keep("", ""))
    }

    @Test
    fun `a duty not heard in a week is dropped`() {
        val day = 24 * 60 * 60_000L
        val now = 100 * day
        assertTrue(WakeHelperRule.dutyHeld(now, now))
        assertTrue(WakeHelperRule.dutyHeld(now, now - 6 * day))
        assertTrue(WakeHelperRule.dutyHeld(now, now - WakeHelperRule.DUTY_KEEP_MS))
        assertFalse(WakeHelperRule.dutyHeld(now, now - WakeHelperRule.DUTY_KEEP_MS - 1))
        assertFalse("never heard", WakeHelperRule.dutyHeld(now, 0))
        assertTrue("the clock went back: held, and told again within the hour", WakeHelperRule.dutyHeld(now, now + day))
    }

    @Test
    fun `a withdrawn duty is read from the word`() {
        assertTrue(WakeHelperRule.withdrawn(JSONObject("""{"computer":"c","withdrawn":true}""")))
        assertFalse(WakeHelperRule.withdrawn(JSONObject("""{"computer":"c","mac":"x"}""")))
        assertFalse(WakeHelperRule.withdrawn(JSONObject("""{"computer":"c","refused":true}""")))
    }

    @Test
    fun `a refusal is read from the duty`() {
        assertTrue(WakeHelperRule.refused(JSONObject("""{"computer":"c","refused":true}""")))
        assertFalse(WakeHelperRule.refused(JSONObject("""{"computer":"c","mac":"x"}""")))
    }

    @Test
    fun `an info names a helper or none`() {
        val some = """{"computer":"c","helper":{"iroh":"aa","relays":[],"name":"Spare phone"}}"""
        val none = """{"computer":"c","helper":null}"""
        assertTrue(WakeHelperRule.helperKnown(some))
        assertEquals("Spare phone", WakeHelperRule.helperName(some))
        assertFalse(WakeHelperRule.helperKnown(none))
        assertEquals("", WakeHelperRule.helperName(none))
        assertFalse(WakeHelperRule.helperKnown(null))
        assertFalse(WakeHelperRule.helperKnown("not json"))
    }

    @Test
    fun `what the helper answers becomes what the wake says`() {
        assertEquals(1, WakeHelperRule.outcome("sent").getOrNull())
        for (word in listOf("slow_down", "unknown", "failed", "anything else")) {
            assertTrue(word, WakeHelperRule.outcome(word).isFailure)
        }
        assertTrue(WakeHelperRule.outcome("slow_down").exceptionOrNull()!!.message!!.contains("half a minute"))
    }
}
