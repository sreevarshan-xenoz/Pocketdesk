package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationForwardRuleTest {
    private val own = "dev.pocketdesk.android"
    private val chat = setOf("com.chat")

    private fun note(
        key: String = "k1",
        pkg: String = "com.chat",
        title: String? = "Ana",
        text: String? = "See you at 6",
        ongoing: Boolean = false,
        groupSummary: Boolean = false,
        visibility: Int = NotificationForwardRule.VISIBILITY_PUBLIC,
        publicTitle: String? = null,
        publicText: String? = null,
        localOnly: Boolean = false,
        otherProfile: Boolean = false,
    ) = PostedNote(
        key, pkg, "Chat", ongoing, groupSummary, title, text, visibility, publicTitle, publicText,
        localOnly, otherProfile, clearable = true,
    )

    @Test
    fun anOrdinaryNotificationOfASharedAppIsSentWithItsAppTitleAndText() {
        val rule = NotificationForwardRule(own)
        assertEquals(
            Outgoing("k1", "Chat", "Ana", "See you at 6", clearable = true),
            rule.decide(note(), enabled = true, shared = chat),
        )
    }

    @Test
    fun aNotificationWithACodeReplacedIsNotSentInFull() {
        val rule = NotificationForwardRule(own)
        val out = rule.decide(note(text = "Your verification code is 482913"), enabled = true, shared = chat)!!
        assertEquals(NotificationForwardRule.CODE_HIDDEN, out.text)
        assertFalse(out.full)
        assertTrue(rule.decide(note(key = "k2"), enabled = true, shared = chat)!!.full)
    }

    @Test
    fun nothingIsSentWhileSharingIsOffOrForAnAppNotSwitchedOn() {
        val rule = NotificationForwardRule(own)
        assertNull(rule.decide(note(), enabled = false, shared = chat))
        assertNull("apps are off until switched on", rule.decide(note(), enabled = true, shared = emptySet()))
    }

    @Test
    fun thisAppOngoingSummariesEmptyLocalOnlyAndOtherProfilesAreNeverSent() {
        val rule = NotificationForwardRule(own)
        val on = { n: PostedNote -> rule.decide(n, true, chat + own) }
        assertNull("this app", on(note(pkg = own)))
        assertNull("ongoing", on(note(key = "o", ongoing = true)))
        assertNull("a group summary", on(note(key = "s", groupSummary = true)))
        assertNull("nothing to show", on(note(key = "e", title = " ", text = null)))
        assertNull("marked for this device only", on(note(key = "l", localOnly = true)))
        assertNull("a work profile's", on(note(key = "w", otherProfile = true)))
    }

    @Test
    fun lockScreenVisibilityIsRespected() {
        val rule = NotificationForwardRule(own)
        assertNull(
            "secret",
            rule.decide(note(key = "a", visibility = NotificationForwardRule.VISIBILITY_SECRET), true, chat),
        )
        assertEquals(
            "private, with a public version: that version",
            Outgoing("b", "Chat", "Chat", "1 new message", clearable = true, full = false),
            rule.decide(
                note(
                    key = "b",
                    visibility = NotificationForwardRule.VISIBILITY_PRIVATE,
                    publicTitle = "Chat",
                    publicText = "1 new message",
                ),
                true,
                chat,
            ),
        )
        assertEquals(
            "private, without one: only that there is one",
            Outgoing("c", "Chat", NotificationForwardRule.HIDDEN_TITLE, "", clearable = true, full = false),
            rule.decide(note(key = "c", visibility = NotificationForwardRule.VISIBILITY_PRIVATE), true, chat),
        )
    }

    @Test
    fun theStrictestOfTheNotificationItsChannelAndTheSystemDecides() {
        val public = NotificationForwardRule.VISIBILITY_PUBLIC
        val private = NotificationForwardRule.VISIBILITY_PRIVATE
        val secret = NotificationForwardRule.VISIBILITY_SECRET
        val none = NotificationForwardRule.NO_OVERRIDE
        val eff = NotificationForwardRule::effectiveVisibility
        assertEquals(public, eff(public, none, none, false))
        assertEquals("the channel hides it", private, eff(public, private, none, false))
        assertEquals("the user's override", secret, eff(public, none, secret, false))
        assertEquals("the system hides sensitive content", private, eff(public, none, none, true))
        assertEquals("never looser than the app asked", secret, eff(secret, public, public, false))
        // Android hides a private notification's content only when the user
        // hides sensitive content (most apps post private, the default).
        assertEquals("an app's private follows the lock screen", public, eff(private, none, none, false))
        assertEquals(private, eff(private, none, none, true))
        assertEquals("the user's private for this app", private, eff(private, none, private, false))
    }

    @Test
    fun aOneTimeCodeStaysOnThePhone() {
        val rule = NotificationForwardRule(own)
        val sent = rule.decide(note(text = "Your verification code is 482913. Do not share it."), true, chat)
        assertEquals(NotificationForwardRule.CODE_HIDDEN, sent?.text)
        assertTrue(NotificationForwardRule.looksLikeCode("G-482913 is your Google verification code"))
        assertTrue(NotificationForwardRule.looksLikeCode("OTP: 5521"))
        assertFalse("a time is not a code", NotificationForwardRule.looksLikeCode("See you at 1830"))
        assertFalse("a word without digits", NotificationForwardRule.looksLikeCode("New code review"))
    }

    @Test
    fun anUpdateThatChangesNothingShownIsNotSentAgain() {
        val rule = NotificationForwardRule(own)
        assertEquals("k1", rule.decide(note(), true, chat)?.id)
        assertNull("progress ticking under the same text", rule.decide(note(), true, chat))
        assertEquals("new text", "k1", rule.decide(note(text = "Running late"), true, chat)?.id)
        rule.removed("k1")
        assertEquals("posted again after it was removed", "k1", rule.decide(note(text = "Running late"), true, chat)?.id)
    }

    @Test
    fun onlyANotificationTheDesktopWasShownIsClosedThere() {
        val rule = NotificationForwardRule(own)
        rule.decide(note(key = "sent"), true, chat)
        rule.decide(note(key = "kept", pkg = "com.bank"), true, chat)
        assertTrue("sent: the desktop closes it", rule.removed("sent"))
        assertFalse("never sent: the desktop hears nothing", rule.removed("kept"))
        assertFalse("closed once", rule.removed("sent"))
    }

    @Test
    fun theMemoryOfWhatWasSentStaysBounded() {
        val rule = NotificationForwardRule(own)
        repeat(NotificationForwardRule.REMEMBERED + 1) { rule.decide(note(key = "k$it"), true, chat) }
        assertEquals("the oldest is forgotten", "k0", rule.decide(note(key = "k0"), true, chat)?.id)
        assertNull("the newest is still remembered", rule.decide(note(key = "k${NotificationForwardRule.REMEMBERED}"), true, chat))
    }
}
