package dev.pocketdesk.android.status

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PhoneStateRuleTest {
    private val mb = 1024L * 1024L

    @Test
    fun storageIsInWholeMbTheFreeRoundedDown() {
        assertEquals(12_300L to 128_000L, PhoneStateRule.storage(12_345 * mb, 128_000 * mb))
        assertEquals(0L to 64L, PhoneStateRule.storage(64 * mb, 64 * mb + 1))
        assertNull(PhoneStateRule.storage(5, 0))
        assertNull(PhoneStateRule.storage(-1, 100 * mb))
        assertNull(PhoneStateRule.storage(200 * mb, 100 * mb))
    }

    @Test
    fun barsComeFromWifisRssiOrTheModemsLevel() {
        assertEquals(4, PhoneStateRule.wifiBars(-40))
        assertEquals(3, PhoneStateRule.wifiBars(-60))
        assertEquals(2, PhoneStateRule.wifiBars(-77))
        assertEquals(1, PhoneStateRule.wifiBars(-80))
        assertEquals(0, PhoneStateRule.wifiBars(-100))
        assertNull("unspecified", PhoneStateRule.wifiBars(Int.MIN_VALUE))
        assertNull(PhoneStateRule.wifiBars(5))
        assertEquals(2, PhoneStateRule.mobileBars(2))
        assertNull(PhoneStateRule.mobileBars(5))
        assertNull(PhoneStateRule.mobileBars(-1))
    }

    @Test
    fun onlyTheNetworksKindOrTheRingerIsSentAtOnce() {
        val wifi = PhoneStateRule.State(storageFreeMb = 100, network = PhoneStateRule.WIFI, signal = 3, ringer = PhoneStateRule.NORMAL)
        assertFalse("the bars", PhoneStateRule.changedNow(wifi, wifi.copy(signal = 2)))
        assertFalse("storage", PhoneStateRule.changedNow(wifi, wifi.copy(storageFreeMb = 200)))
        assertTrue("the network", PhoneStateRule.changedNow(wifi, wifi.copy(network = PhoneStateRule.MOBILE)))
        assertTrue("the ringer", PhoneStateRule.changedNow(wifi, wifi.copy(ringer = PhoneStateRule.SILENT)))
    }

    @Test
    fun theFieldsAreWhatWasRead() {
        assertEquals("", PhoneStateRule.fields(PhoneStateRule.State()))
        assertEquals(
            ""","storageFreeMb":12300,"storageTotalMb":128000,"network":"wifi","signal":3,"ringer":"silent"""",
            PhoneStateRule.fields(
                PhoneStateRule.State(12_300, 128_000, PhoneStateRule.WIFI, 3, PhoneStateRule.SILENT),
            ),
        )
        assertEquals(
            ""","network":"none"""",
            PhoneStateRule.fields(PhoneStateRule.State(network = PhoneStateRule.NONE)),
        )
    }
}
