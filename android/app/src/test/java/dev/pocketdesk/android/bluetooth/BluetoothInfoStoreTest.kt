package dev.pocketdesk.android.bluetooth

import dev.pocketdesk.android.FakeContext
import dev.pocketdesk.android.HostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Where to dial a saved computer by Bluetooth is kept with its other records. */
class BluetoothInfoStoreTest {
    private val ctx = FakeContext()
    private val a = HostStore.Host("192.0.2.1", HostStore.DEFAULT_STREAMING_PORT)
    private val b = HostStore.Host("192.0.2.2", HostStore.DEFAULT_STREAMING_PORT)
    private val said =
        """{"address":"00:00:5E:00:53:01","service":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13","link":"b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12"}"""

    @Test
    fun kept_for_a_saved_computer_and_moved_and_forgotten_with_it() {
        HostStore.save(ctx, a)
        assertNull(HostStore.bluetoothInfo(ctx, a.address))
        HostStore.saveBluetoothInfo(ctx, a.address, said)
        assertEquals(said, HostStore.bluetoothInfo(ctx, a.address))
        HostStore.move(ctx, a.address, b)
        assertNull(HostStore.bluetoothInfo(ctx, a.address))
        assertEquals(said, HostStore.bluetoothInfo(ctx, b.address))
        HostStore.remove(ctx, b.address)
        assertNull(HostStore.bluetoothInfo(ctx, b.address))
    }

    @Test
    fun nothing_is_kept_for_an_address_that_is_not_saved() {
        HostStore.saveBluetoothInfo(ctx, a.address, said)
        assertNull(HostStore.bluetoothInfo(ctx, a.address))
    }
}
