package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NotificationForwardRuleTest {
    private val own = "dev.pocketdesk.android"

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
    ) = PostedNote(key, pkg, "Chat", ongoing, groupSummary, title, text, visibility, publicTitle, publicText)

    @Test
    fun anOrdinaryNotificationIsSentWithItsAppTitleAndText() {
        val rule = NotificationForwardRule(own)
        assertEquals(
            Outgoing("k1", "Chat", "Ana", "See you at 6"),
            rule.decide(note(), enabled = true, excluded = emptySet()),
        )
    }

    @Test
    fun nothingIsSentWhileSharingIsOff() {
        assertNull(NotificationForwardRule(own).decide(note(), enabled = false, excluded = emptySet()))
    }

    @Test
    fun thisAppOngoingSummariesEmptyAndExcludedAppsAreNeverSent() {
        val rule = NotificationForwardRule(own)
        val on = { n: PostedNote, excluded: Set<String> -> rule.decide(n, true, excluded) }
        assertNull("this app", on(note(pkg = own), emptySet()))
        assertNull("ongoing", on(note(key = "o", ongoing = true), emptySet()))
        assertNull("a group summary", on(note(key = "s", groupSummary = true), emptySet()))
        assertNull("nothing to show", on(note(key = "e", title = " ", text = null), emptySet()))
        assertNull("an excluded app", on(note(key = "x"), setOf("com.chat")))
    }

    @Test
    fun lockScreenVisibilityIsRespected() {
        val rule = NotificationForwardRule(own)
        assertNull(
            "secret",
            rule.decide(note(key = "a", visibility = NotificationForwardRule.VISIBILITY_SECRET), true, emptySet()),
        )
        assertEquals(
            "private, with a public version: that version",
            Outgoing("b", "Chat", "Chat", "1 new message"),
            rule.decide(
                note(
                    key = "b",
                    visibility = NotificationForwardRule.VISIBILITY_PRIVATE,
                    publicTitle = "Chat",
                    publicText = "1 new message",
                ),
                true,
                emptySet(),
            ),
        )
        assertEquals(
            "private, without one: only that there is one",
            Outgoing("c", "Chat", NotificationForwardRule.HIDDEN_TITLE, ""),
            rule.decide(note(key = "c", visibility = NotificationForwardRule.VISIBILITY_PRIVATE), true, emptySet()),
        )
    }

    @Test
    fun anUpdateThatChangesNothingShownIsNotSentAgain() {
        val rule = NotificationForwardRule(own)
        assertEquals("k1", rule.decide(note(), true, emptySet())?.id)
        assertNull("progress ticking under the same text", rule.decide(note(), true, emptySet()))
        assertEquals("new text", "k1", rule.decide(note(text = "Running late"), true, emptySet())?.id)
        rule.removed("k1")
        assertEquals("posted again after it was removed", "k1", rule.decide(note(text = "Running late"), true, emptySet())?.id)
    }

    @Test
    fun theMemoryOfWhatWasSentStaysBounded() {
        val rule = NotificationForwardRule(own)
        repeat(NotificationForwardRule.REMEMBERED + 10) { rule.decide(note(key = "k$it"), true, emptySet()) }
        assertEquals("the oldest is forgotten", "k0", rule.decide(note(key = "k0"), true, emptySet())?.id)
    }
}
