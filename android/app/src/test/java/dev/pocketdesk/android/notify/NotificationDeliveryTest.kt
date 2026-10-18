package dev.pocketdesk.android.notify

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationDeliveryTest {
    @Test
    fun `a notification the computer read, or refused, is done with`() {
        assertTrue(NotificationDelivery.sent("acked"))
        assertTrue(NotificationDelivery.sent("refused"))
    }

    @Test
    fun `silence from a computer that answers means the link died, so it waits`() {
        assertFalse(NotificationDelivery.sent("unanswered"))
    }

    @Test
    fun `one not asked about, or sent on a connection of its own, went`() {
        assertTrue(NotificationDelivery.sent("sent"))
        assertTrue(NotificationDelivery.sent("pushed"))
    }

    @Test
    fun `an outcome it does not know is not taken as sent`() {
        assertFalse(NotificationDelivery.sent(""))
    }
}
