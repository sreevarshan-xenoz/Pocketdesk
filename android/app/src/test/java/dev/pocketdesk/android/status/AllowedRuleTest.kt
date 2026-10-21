package dev.pocketdesk.android.status

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AllowedRuleTest {
    private val allOn = SwitchReport.FEATURES.associateWith { AllowedRule.Side(permission = true, setting = true) }
    private val phoneOn = SwitchReport.FEATURES.associateWith { SwitchReport.Switch(on = true, android = true) }

    @Test
    fun `every feature in the phone's order, working when both sides allow it`() {
        val rows = AllowedRule.rows(allOn, phoneOn)
        assertEquals(SwitchReport.FEATURES, rows.map { it.feature })
        assertTrue(rows.all { it.works })
    }

    @Test
    fun `the computer's permission is said before its Settings`() {
        val allowed = allOn + mapOf(
            "files" to AllowedRule.Side(permission = false, setting = true),
            "clipboard" to AllowedRule.Side(permission = true, setting = false),
            "control" to AllowedRule.Side(permission = false, setting = false),
        )
        val rows = AllowedRule.rows(allowed, phoneOn).associateBy { it.feature }
        assertEquals(AllowedRule.Computer.PERMISSION_OFF, rows.getValue("files").computer)
        assertEquals(AllowedRule.Computer.SETTING_OFF, rows.getValue("clipboard").computer)
        assertEquals(AllowedRule.Computer.PERMISSION_OFF, rows.getValue("control").computer)
        assertFalse(rows.getValue("files").works)
    }

    @Test
    fun `the phone's own off, and Android refusing`() {
        val switches = phoneOn + mapOf(
            "ring" to SwitchReport.Switch(on = false, android = true),
            "texts" to SwitchReport.Switch(on = true, android = false),
            "links" to SwitchReport.Switch(on = true, android = null),
        )
        val rows = AllowedRule.rows(allOn, switches).associateBy { it.feature }
        assertEquals(AllowedRule.Phone.OFF, rows.getValue("ring").phone)
        assertEquals(AllowedRule.Phone.ANDROID_REFUSES, rows.getValue("texts").phone)
        assertEquals(AllowedRule.Phone.ON, rows.getValue("links").phone)
        assertFalse(rows.getValue("texts").works)
    }

    @Test
    fun `a feature the computer did not answer for is unknown, and works nowhere`() {
        val rows = AllowedRule.rows(emptyMap(), phoneOn)
        assertTrue(rows.all { it.computer == AllowedRule.Computer.UNKNOWN && !it.works })
    }

    @Test
    fun `a feature the phone did not read is off on the phone`() {
        val rows = AllowedRule.rows(allOn, emptyMap())
        assertTrue(rows.all { it.phone == AllowedRule.Phone.OFF })
    }

    @Test
    fun `a held-back feature the phone wants is flagged, one it turned off is not`() {
        val allowed = allOn + ("files" to AllowedRule.Side(permission = false, setting = true))
        val switches = phoneOn + ("files" to SwitchReport.Switch(on = false, android = true))
        assertFalse(AllowedRule.rows(allowed, switches).single { it.feature == "files" }.heldBack)
        assertTrue(AllowedRule.rows(allowed, phoneOn).single { it.feature == "files" }.heldBack)
    }
}
