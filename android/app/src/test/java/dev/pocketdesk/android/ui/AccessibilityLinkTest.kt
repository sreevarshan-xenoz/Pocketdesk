package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.AccessibilityLink.Switch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AccessibilityLinkTest {
    @Test
    fun `the link goes under the first switch on the page that needs the service`() {
        val on = mapOf(Switch.CONTROL to true, Switch.TYPING to true, Switch.COPIES to true)
        assertEquals(Switch.CONTROL, AccessibilityLink.shownUnder(on, serviceOn = false))
    }

    @Test
    fun `with the first switch off it moves down to the next one on`() {
        val on = mapOf(Switch.CONTROL to false, Switch.TYPING to false, Switch.PHONE_ACTIONS to true, Switch.COPIES to true)
        assertEquals(Switch.PHONE_ACTIONS, AccessibilityLink.shownUnder(on, serviceOn = false))
    }

    @Test
    fun `no link with the service on, or with every switch off`() {
        assertNull(AccessibilityLink.shownUnder(mapOf(Switch.CONTROL to true), serviceOn = true))
        assertNull(AccessibilityLink.shownUnder(emptyMap(), serviceOn = false))
    }
}
