package dev.pocketdesk.android.notify

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationDeliveryTest {
    @Test
    fun `a notification the computer read, or refused, is done with`() {
        assertTrue(NotificationDelivery.sent("acked", computerAnswers = true))
        assertTrue(NotificationDelivery.sent("refused", computerAnswers = true))
        assertTrue(NotificationDelivery.answered("acked"))
        assertTrue(NotificationDelivery.answered("refused"))
    }

    @Test
    fun `silence from a computer that answers means the link died, so it waits`() {
        assertFalse(NotificationDelivery.sent("unanswered", computerAnswers = true))
    }

    @Test
    fun `silence from a computer that never answered is taken as sent, as before`() {
        assertTrue(NotificationDelivery.sent("unanswered", computerAnswers = false))
        assertFalse(NotificationDelivery.answered("unanswered"))
    }

    @Test
    fun `one sent on a connection of its own went`() {
        assertTrue(NotificationDelivery.sent("pushed", computerAnswers = true))
        assertFalse(NotificationDelivery.answered("pushed"))
    }

    @Test
    fun `an outcome it does not know is not taken as sent`() {
        assertFalse(NotificationDelivery.sent("", computerAnswers = false))
    }
}
