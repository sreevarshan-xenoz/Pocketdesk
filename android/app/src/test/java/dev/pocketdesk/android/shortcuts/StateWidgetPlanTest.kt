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
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Tap, tried = true, auto = false, metered = true))
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, tried = false, auto = false, metered = true))
        assertTrue(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, tried = true, auto = true, metered = false))
        assertFalse(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, tried = true, auto = false, metered = false))
        // The widget's own refresh never dials over mobile data (G1 review).
        assertFalse(StateWidgetPlan.ask(StateWidgetPlan.Why.Period, tried = true, auto = true, metered = true))
        // A computer that never answered was tried: not asked again at each period (G1 review).
        assertTrue(StateWidgetPlan.tried(json = null, failed = true))
        assertTrue(StateWidgetPlan.tried(json = "{}", failed = false))
        assertFalse(StateWidgetPlan.tried(json = null, failed = false))
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
        assertEquals(StateWidgetPlan.When.Updated, StateWidgetPlan.timeLine(readAt = 5L, failed = false, asking = false))
        assertEquals(StateWidgetPlan.When.StaleSince, StateWidgetPlan.timeLine(readAt = 5L, failed = true, asking = false))
        assertEquals(StateWidgetPlan.When.NotAnswering, StateWidgetPlan.timeLine(readAt = null, failed = true, asking = false))
        assertEquals(StateWidgetPlan.When.Asking, StateWidgetPlan.timeLine(readAt = null, failed = false, asking = false))
        // While an ask runs the widget says so, over whatever it shows (G1 review).
        assertEquals(StateWidgetPlan.When.Asking, StateWidgetPlan.timeLine(readAt = 5L, failed = true, asking = true))
    }

    /** I2: bars once there is room for them, the one line below that. */
    @Test
    fun aTallWidgetShowsBarsAndASmallOneTheLine() {
        assertEquals(StateWidgetPlan.Layout.Line, StateWidgetPlan.layout(heightDp = 80))
        assertEquals(StateWidgetPlan.Layout.Line, StateWidgetPlan.layout(heightDp = 109))
        assertEquals(StateWidgetPlan.Layout.Bars, StateWidgetPlan.layout(heightDp = 110))
        assertEquals(StateWidgetPlan.Layout.Bars, StateWidgetPlan.layout(heightDp = 300))
        // A launcher that never said: the line, which fits anywhere.
        assertEquals(StateWidgetPlan.Layout.Line, StateWidgetPlan.layout(heightDp = 0))
    }

    /** I2 review: a launcher's height for the phone held as it is now. */
    @Test
    fun theHeightIsThePortraitOrLandscapeOne() {
        // Portrait: the widget's tallest; landscape: its shortest.
        assertEquals(180, StateWidgetPlan.heightDp(portrait = true, minHeightDp = 90, maxHeightDp = 180))
        assertEquals(90, StateWidgetPlan.heightDp(portrait = false, minHeightDp = 90, maxHeightDp = 180))
        assertEquals(StateWidgetPlan.Layout.Line, StateWidgetPlan.layout(StateWidgetPlan.heightDp(false, 90, 180)))
    }

    @Test
    fun theBarsAreTheReadingsTheComputerGave() {
        val rows = StateWidgetPlan.rows(StateWidgetPlan.Numbers(cpu = 10, memory = null, disk = 84, battery = null))
        assertEquals(
            listOf(StateWidgetPlan.Row(StateWidgetPlan.Part.Cpu, 10), StateWidgetPlan.Row(StateWidgetPlan.Part.Disk, 84)),
            rows,
        )
        assertEquals(4, StateWidgetPlan.rows(StateWidgetPlan.Numbers(1, 2, 3, 4)).size)
    }

    @Test
    fun aComputerNotAnsweringIsAWarning() {
        assertTrue(StateWidgetPlan.warns(StateWidgetPlan.When.NotAnswering))
        assertTrue(StateWidgetPlan.warns(StateWidgetPlan.When.StaleSince))
        assertFalse(StateWidgetPlan.warns(StateWidgetPlan.When.Updated))
        assertFalse(StateWidgetPlan.warns(StateWidgetPlan.When.Asking))
    }

    @Test
    fun aReadingIsKeptOnlyForItsComputer() {
        assertTrue(StateWidgetPlan.keeps(readFor = "a", lastComputer = "a"))
        assertFalse(StateWidgetPlan.keeps(readFor = "a", lastComputer = "b"))
        assertFalse(StateWidgetPlan.keeps(readFor = null, lastComputer = "a"))
        assertFalse(StateWidgetPlan.keeps(readFor = "a", lastComputer = null))
    }
}
