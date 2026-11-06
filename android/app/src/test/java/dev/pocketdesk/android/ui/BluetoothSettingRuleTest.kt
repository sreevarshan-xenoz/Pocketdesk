package dev.pocketdesk.android.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BluetoothSettingRuleTest {
    /** Refused, and Android will not ask again: the app's own settings page opens. */
    @Test
    fun a_permission_refused_for_good_goes_to_the_apps_page() {
        assertTrue(BluetoothSettingRule.deniedForGood(granted = false, rationale = false))
        // Refused once: Android asks again next time.
        assertFalse(BluetoothSettingRule.deniedForGood(granted = false, rationale = true))
        assertFalse(BluetoothSettingRule.deniedForGood(granted = true, rationale = false))
    }
}
