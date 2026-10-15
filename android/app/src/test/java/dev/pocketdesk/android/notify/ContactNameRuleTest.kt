package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ContactNameRuleTest {
    @Test
    fun aNameIsSentOnOneTrimmedLineOrNotAtAll() {
        assertEquals("Ann Lee", ContactNameRule.clean("  Ann\nLee "))
        assertEquals("B".repeat(ContactNameRule.MAX_CHARS), ContactNameRule.clean("B".repeat(100)))
        assertNull(ContactNameRule.clean("   "))
        assertNull(ContactNameRule.clean(null))
    }

    @Test
    fun aNameIsLookedUpOnlyWithTheSettingAndThePermissionAndForANumber() {
        assertEquals(true, ContactNameRule.shouldLookUp(settingOn = true, granted = true, address = "+1 555 0100"))
        assertEquals(false, ContactNameRule.shouldLookUp(settingOn = true, granted = false, address = "+1555"))
        assertEquals(false, ContactNameRule.shouldLookUp(settingOn = false, granted = true, address = "+1555"))
        // A sender ID with letters is no number: Android would match its
        // keypad digits to an unrelated contact.
        assertEquals(false, ContactNameRule.shouldLookUp(settingOn = true, granted = true, address = "BANK"))
    }

    @Test
    fun aLongNameIsCutBetweenCharactersNotInsideOne() {
        // An emoji (two UTF-16 units) at the cut is kept whole or left out.
        val emoji = String(Character.toChars(0x1F600))
        val name = "A".repeat(ContactNameRule.MAX_CHARS - 1) + emoji + "B"
        val cut = ContactNameRule.clean(name)!!
        assertEquals(false, cut.last().isHighSurrogate())
        assertEquals(ContactNameRule.MAX_CHARS, cut.codePointCount(0, cut.length))
    }
}
