package dev.pocketdesk.android.ring

import org.junit.Assert.assertEquals
import org.junit.Test

class RingRuleTest {
    @Test
    fun aRingRingsWithTheSwitchOnAndIsDroppedWithItOff() {
        val rule = RingRule()
        assertEquals(RingRule.Act.NOTHING, rule.decide(asked = false, on = true, inSession = false, nowMs = 0))
        assertEquals(RingRule.Act.TAKE_AND_DROP, rule.decide(asked = true, on = false, inSession = false, nowMs = 0))
        assertEquals(RingRule.Act.RING, rule.decide(asked = true, on = true, inSession = false, nowMs = 1_000))
    }

    @Test
    fun aSessionRingsForItself() {
        assertEquals(RingRule.Act.NOTHING, RingRule().decide(asked = true, on = true, inSession = true, nowMs = 0))
    }

    @Test
    fun atMostOnceAMinute() {
        val rule = RingRule()
        assertEquals(RingRule.Act.RING, rule.decide(true, true, false, 10_000))
        assertEquals(RingRule.Act.TAKE_AND_DROP, rule.decide(true, true, false, 40_000))
        assertEquals(RingRule.Act.RING, rule.decide(true, true, false, 70_000))
    }
}
