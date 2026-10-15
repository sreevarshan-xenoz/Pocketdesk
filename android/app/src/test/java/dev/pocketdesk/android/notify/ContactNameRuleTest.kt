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
    fun aNameIsLookedUpOnlyWithTheSettingAndThePermission() {
        assertEquals(true, ContactNameRule.shouldLookUp(settingOn = true, granted = true))
        assertEquals(false, ContactNameRule.shouldLookUp(settingOn = true, granted = false))
        assertEquals(false, ContactNameRule.shouldLookUp(settingOn = false, granted = true))
    }
}
