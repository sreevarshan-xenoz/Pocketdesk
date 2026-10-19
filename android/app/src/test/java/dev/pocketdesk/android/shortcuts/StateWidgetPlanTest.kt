package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.ComputerState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class StateWidgetPlanTest {
    @Test
    fun aTapOrAFirstDrawAlwaysAsksAndThePeriodOnlyWhenAllowed() {
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Tap, hasReading = true, auto = false))
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, hasReading = false, auto = false))
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, hasReading = true, auto = true))
        assertFalse(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, hasReading = true, auto = false))
    }

    @Test
    fun theNumbersAreWholePercentsWithTheFullestDisk() {
        val state = ComputerState.State(
            cpu = 12.4f,
            memory = ComputerState.Space("", 48, 100),
            disks = listOf(ComputerState.Space("C:", 30, 100), ComputerState.Space("D:", 71, 100)),
            batteryCharge = 80,
        )
        val numbers = StateWidgetPlan.numbers(state)
        assertEquals(12, numbers.cpu)
        assertEquals(48, numbers.memory)
        assertEquals(71, numbers.disk)
        assertEquals(80, numbers.battery)

        val bare = StateWidgetPlan.numbers(ComputerState.State())
        assertNull(bare.cpu)
        assertNull(bare.memory)
        assertNull(bare.disk)
        assertNull(bare.battery)
        assertEquals(100, StateWidgetPlan.numbers(ComputerState.State(cpu = 99.6f)).cpu)
    }

    @Test
    fun theTimeLineSaysWhetherTheLastTryAnswered() {
        assertEquals(StateWidgetPlan.When.Updated, StateWidgetPlan.timeLine(readAt = 5L, failed = false))
        assertEquals(StateWidgetPlan.When.StaleSince, StateWidgetPlan.timeLine(readAt = 5L, failed = true))
        assertEquals(StateWidgetPlan.When.NotAnswering, StateWidgetPlan.timeLine(readAt = null, failed = true))
        assertEquals(StateWidgetPlan.When.Asking, StateWidgetPlan.timeLine(readAt = null, failed = false))
    }

    @Test
    fun aReadingIsKeptOnlyForItsComputer() {
        assertTrue(StateWidgetPlan.keeps(readFor = "a", lastComputer = "a"))
        assertFalse(StateWidgetPlan.keeps(readFor = "a", lastComputer = "b"))
        assertFalse(StateWidgetPlan.keeps(readFor = null, lastComputer = "a"))
        assertFalse(StateWidgetPlan.keeps(readFor = "a", lastComputer = null))
    }
}
