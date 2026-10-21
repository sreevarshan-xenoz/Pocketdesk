package dev.pocketdesk.android.status

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SwitchReportTest {
    private val on = SwitchReport.Switch(on = true, android = null)

    @Test
    fun theBodyHasOnlyKnownFeaturesInTheirOrderAndAndroidWhereItCounts() {
        val body = SwitchReport.body(
            mapOf(
                "links" to SwitchReport.Switch(on = false, android = null),
                "notifications" to SwitchReport.Switch(on = true, android = false),
                "com.example.app" to on,
            ),
        )
        assertEquals(
            """{"switches":{"notifications":{"on":true,"android":false},"links":{"on":false}}}""",
            body,
        )
        assertEquals("""{"switches":{}}""", SwitchReport.body(emptyMap()))
    }

    @Test
    fun itIsSentAtTheStartOnAChangeAndWithTheBatterysResend() {
        val now = mapOf("links" to on)
        assertTrue("the first", SwitchReport.shouldSend(null, 0, now, 0))
        assertFalse("the same, soon", SwitchReport.shouldSend(now, 0, now, 60_000))
        assertTrue("a change", SwitchReport.shouldSend(now, 0, mapOf("links" to on.copy(on = false)), 1_000))
        assertTrue("in time", SwitchReport.shouldSend(now, 0, now, BatteryRule.RESEND_MS))
    }

    @Test
    fun theSenderKeepsWhatWentAndTriesAgainAfterAFailure() {
        val sender = SwitchSender()
        val now = mapOf("links" to on)
        var sent = 0
        sender.tick(now, 0) { Result.failure<Unit>(IllegalStateException("no link")) }
        sender.tick(now, 1_000) { sent++; Result.success(Unit) }
        sender.tick(now, 2_000) { sent++; Result.success(Unit) }
        assertEquals("sent once it went", 1, sent)
        sender.forget()
        sender.tick(now, 3_000) { sent++; Result.success(Unit) }
        assertEquals("told again on a new link", 2, sent)
    }
}
