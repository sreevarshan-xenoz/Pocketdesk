package dev.pocketdesk.android.bluetooth

import dev.pocketdesk.android.FakeContext
import dev.pocketdesk.android.Prefs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BluetoothNoticeTest {
    private val ctx = FakeContext()

    @Test
    fun told_once() {
        assertFalse(Prefs.bluetoothTold(ctx))
        // Not on another road.
        assertNull(BluetoothNotice.next(ctx, road = 0, inFront = true, canNotify = true))
        assertNull(BluetoothNotice.next(ctx, road = 1, inFront = true, canNotify = true))
        assertFalse(Prefs.bluetoothTold(ctx))
        // The first time on Bluetooth: a dialog with the app in front.
        assertEquals(BluetoothNotice.Way.DIALOG, BluetoothNotice.next(ctx, road = 2, inFront = true, canNotify = true))
        assertTrue(Prefs.bluetoothTold(ctx))
        // Never again, in front or not.
        assertNull(BluetoothNotice.next(ctx, road = 2, inFront = true, canNotify = true))
        assertNull(BluetoothNotice.next(ctx, road = 2, inFront = false, canNotify = true))
    }

    @Test
    fun a_notification_when_the_app_is_away_and_nothing_while_none_can_show() {
        // Notifications off and the app away: nothing is told yet, so the
        // next time the app is in front it is.
        assertNull(BluetoothNotice.next(ctx, road = 2, inFront = false, canNotify = false))
        assertFalse(Prefs.bluetoothTold(ctx))
        assertEquals(BluetoothNotice.Way.NOTIFICATION, BluetoothNotice.next(ctx, road = 2, inFront = false, canNotify = true))
        assertTrue(Prefs.bluetoothTold(ctx))
        assertNull(BluetoothNotice.next(ctx, road = 2, inFront = true, canNotify = true))
    }
}
