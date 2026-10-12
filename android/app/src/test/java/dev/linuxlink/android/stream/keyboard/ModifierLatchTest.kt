package dev.linuxlink.android.stream.keyboard

import org.junit.Assert.assertEquals
import org.junit.Test

class ModifierLatchTest {
    @Test
    fun aTapLatchesForOneKey() {
        val latch = ModifierLatch()
        latch.tap(Modifier.Ctrl)
        assertEquals(LatchState.Once, latch.state(Modifier.Ctrl))
        assertEquals(listOf(Modifier.Ctrl), latch.consume())
        assertEquals(LatchState.Off, latch.state(Modifier.Ctrl))
        assertEquals(emptyList<Modifier>(), latch.consume())
    }

    @Test
    fun aSecondTapUnlatches() {
        val latch = ModifierLatch()
        latch.tap(Modifier.Alt)
        latch.tap(Modifier.Alt)
        assertEquals(LatchState.Off, latch.state(Modifier.Alt))
    }

    @Test
    fun aLockLastsUntilTappedOff() {
        val latch = ModifierLatch()
        latch.lock(Modifier.Shift)
        assertEquals(listOf(Modifier.Shift), latch.consume())
        assertEquals(listOf(Modifier.Shift), latch.consume())
        latch.tap(Modifier.Shift)
        assertEquals(LatchState.Off, latch.state(Modifier.Shift))
    }

    @Test
    fun modifiersComeOutInAFixedOrder() {
        val latch = ModifierLatch()
        latch.tap(Modifier.Super)
        latch.lock(Modifier.Shift)
        latch.tap(Modifier.Ctrl)
        assertEquals(listOf(Modifier.Ctrl, Modifier.Shift, Modifier.Super), latch.consume())
        assertEquals("only the lock survives", listOf(Modifier.Shift), latch.active())
    }

    @Test
    fun resetClearsLocksToo() {
        val latch = ModifierLatch()
        var changes = 0
        latch.onChange = { changes++ }
        latch.lock(Modifier.Ctrl)
        latch.reset()
        assertEquals(emptyList<Modifier>(), latch.active())
        assertEquals(2, changes)
    }
}
