package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SmsReplyRuleTest {
    private val request = SmsRequest("r1", "+1 (555) 010-0100", "on my way", "192.0.2.1", 1716)
    private val send = SmsReplyRule.Decision.Send

    private fun decide(
        r: SmsRequest = request,
        replyOn: Boolean = true,
        permitted: Boolean = true,
        fromOwn: Boolean = true,
    ) = SmsReplyRule.decide(r, replyOn, permitted, fromOwn)

    @Test
    fun aReplyIsSentOnlyWhenTheOwnerAllowsItAndAndroidDoes() {
        assertEquals(send, decide())
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.OFF), decide(replyOn = false))
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.NO_PERMISSION), decide(permitted = false))
    }

    @Test
    fun onlyThePhonesOwnComputerMayAskForOne() {
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.NOT_YOURS), decide(fromOwn = false))
    }

    @Test
    fun aNumberMustBeAFullOneNotAShortCode() {
        val bad = SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_NUMBER)
        assertEquals(bad, decide(request.copy(address = "call me")))
        assertEquals(bad, decide(request.copy(address = "+")))
        assertEquals(bad, decide(request.copy(address = "1".repeat(40))))
        assertEquals("a premium short code", bad, decide(request.copy(address = "85433")))
        assertEquals(bad, decide(request.copy(address = "*121#")))
        assertEquals(send, decide(request.copy(address = "5550100")))
    }

    @Test
    fun anEmptyOrHugeTextIsRefused() {
        val bad = SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_TEXT)
        assertEquals(bad, decide(request.copy(body = "  ")))
        assertEquals(bad, decide(request.copy(body = "x".repeat(SmsReplyRule.MAX_TEXT + 1))))
    }

    @Test
    fun atMostFiveRepliesGoOutAMinute() {
        val budget = SmsReplyRule.SendBudget()
        repeat(SmsReplyRule.SendBudget.MAX) { assertTrue(budget.take(1_000L + it)) }
        assertFalse("the sixth in the same minute", budget.take(2_000))
        assertTrue("a minute after the first", budget.take(61_000))
    }

    @Test
    fun partsOfOneIncomingTextAreJoinedPerSender() {
        val joined = SmsReplyRule.joinParts(listOf("+1" to "Hel", "+2" to "Yo", "+1" to "lo"))
        assertEquals(listOf("+1" to "Hello", "+2" to "Yo"), joined)
    }
}
