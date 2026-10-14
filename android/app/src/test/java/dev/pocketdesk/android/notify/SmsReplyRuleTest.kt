package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Test

class SmsReplyRuleTest {
    private val request = SmsRequest("r1", "+1 (555) 010-0100", "on my way")

    @Test
    fun aReplyIsSentOnlyWhenTheOwnerAllowsItAndAndroidDoes() {
        assertEquals(SmsReplyRule.Decision.Send, SmsReplyRule.decide(request, replyOn = true, permitted = true))
        assertEquals(
            SmsReplyRule.Decision.Refuse(SmsReplyRule.OFF),
            SmsReplyRule.decide(request, replyOn = false, permitted = true),
        )
        assertEquals(
            SmsReplyRule.Decision.Refuse(SmsReplyRule.NO_PERMISSION),
            SmsReplyRule.decide(request, replyOn = true, permitted = false),
        )
    }

    @Test
    fun aNumberMustLookLikeOne() {
        val refused = { address: String -> SmsReplyRule.decide(request.copy(address = address), true, true) }
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_NUMBER), refused("call me"))
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_NUMBER), refused("+"))
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_NUMBER), refused("1".repeat(40)))
        assertEquals(SmsReplyRule.Decision.Send, refused("121"))
        assertEquals(SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_NUMBER), refused("*121#"))
    }

    @Test
    fun anEmptyOrHugeTextIsRefused() {
        assertEquals(
            SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_TEXT),
            SmsReplyRule.decide(request.copy(body = "  "), true, true),
        )
        assertEquals(
            SmsReplyRule.Decision.Refuse(SmsReplyRule.BAD_TEXT),
            SmsReplyRule.decide(request.copy(body = "x".repeat(SmsReplyRule.MAX_TEXT + 1)), true, true),
        )
    }

    @Test
    fun partsOfOneIncomingTextAreJoinedPerSender() {
        val joined = SmsReplyRule.joinParts(listOf("+1" to "Hel", "+2" to "Yo", "+1" to "lo"))
        assertEquals(listOf("+1" to "Hello", "+2" to "Yo"), joined)
    }
}
