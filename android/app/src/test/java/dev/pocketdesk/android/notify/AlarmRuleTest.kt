package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AlarmRuleTest {
    private val clocks = setOf("com.coloros.alarmclock")

    @Test
    fun anAlarmIsKnownByItsCategoryOrAsTheClocksNoClearOne() {
        assertTrue(AlarmRule.isAlarm("alarm", "com.google.android.deskclock", emptySet(), noClear = false))
        // ColorOS: no category, FLAG_NO_CLEAR with Snooze and Stop.
        assertTrue(AlarmRule.isAlarm(null, "com.coloros.alarmclock", clocks, noClear = true))
        assertFalse("the upcoming one", AlarmRule.isAlarm(null, "com.coloros.alarmclock", clocks, noClear = false))
        assertFalse("another app's no-clear one", AlarmRule.isAlarm(null, "com.chat", clocks, noClear = true))
        assertFalse(AlarmRule.isAlarm("msg", "com.chat", clocks, noClear = false))
    }

    private fun note(pkg: String, alarm: Boolean, ongoing: Boolean = true, localOnly: Boolean = false) =
        PostedNote(
            key = "k-$pkg",
            pkg = pkg,
            appLabel = "Clock",
            ongoing = ongoing,
            groupSummary = false,
            title = "07:00",
            text = "Alarm ringing",
            visibility = 1,
            publicTitle = null,
            publicText = null,
            localOnly = localOnly,
            otherProfile = false,
            clearable = false,
            alarm = alarm,
        )

    @Test
    fun anAlarmGoesOnlyWithItsSwitchWhateverTheAppsAndOngoing() {
        val rule = NotificationForwardRule("dev.pocketdesk.android")
        val clock = note("com.coloros.alarmclock", alarm = true, localOnly = true)
        assertNull("off: dropped as ongoing and unshared", rule.decide(clock, enabled = true, shared = emptySet()))
        val out = rule.decide(clock, enabled = true, shared = emptySet(), alarmsOn = true)
        assertNotNull(out)
        assertTrue(out!!.alarm)
        assertEquals("07:00", out.title)
        // Not an alarm: the switch changes nothing.
        assertNull(rule.decide(note("com.player", alarm = false), enabled = true, shared = setOf("com.player"), alarmsOn = true))
    }

    @Test
    fun anAlarmIsNeverSentWithTheListenerOff() {
        val rule = NotificationForwardRule("dev.pocketdesk.android")
        assertNull(rule.decide(note("com.coloros.alarmclock", alarm = true), enabled = false, shared = emptySet(), alarmsOn = true))
    }
}
