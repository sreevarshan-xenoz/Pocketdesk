package dev.pocketdesk.android.stream.keyboard

import org.junit.Assert.assertEquals
import org.junit.Test

class ModifierLatchTest {
    @Test
    fun aTapLatchesForOneKey() {
        val latch = ModifierLatch()
        latch.tap(KeyModifier.Ctrl, nowMs = 0)
        assertEquals(LatchState.Once, latch.state(KeyModifier.Ctrl))
        assertEquals(listOf(KeyModifier.Ctrl), latch.consume())
        assertEquals(LatchState.Off, latch.state(KeyModifier.Ctrl))
        assertEquals(emptyList<KeyModifier>(), latch.consume())
    }

    @Test
    fun aSecondTapUnlatches() {
        val latch = ModifierLatch()
        latch.tap(KeyModifier.Alt, nowMs = 0)
        latch.tap(KeyModifier.Alt, nowMs = 1_000)
        assertEquals(LatchState.Off, latch.state(KeyModifier.Alt))
    }

    @Test
    fun aQuickSecondTapLocksAndTheFirstLatchesAtOnce() {
        val latch = ModifierLatch()
        latch.tap(KeyModifier.Ctrl, nowMs = 1_000)
        assertEquals("latched immediately, not after a double-tap timeout", LatchState.Once, latch.state(KeyModifier.Ctrl))
        latch.tap(KeyModifier.Ctrl, nowMs = 1_200)
        assertEquals(LatchState.Locked, latch.state(KeyModifier.Ctrl))
    }

    @Test
    fun aLockLastsUntilTappedOff() {
        val latch = ModifierLatch()
        latch.lock(KeyModifier.Shift)
        assertEquals(listOf(KeyModifier.Shift), latch.consume())
        assertEquals(listOf(KeyModifier.Shift), latch.consume())
        latch.tap(KeyModifier.Shift, nowMs = 5_000)
        assertEquals(LatchState.Off, latch.state(KeyModifier.Shift))
    }

    @Test
    fun modifiersComeOutInAFixedOrder() {
        val latch = ModifierLatch()
        latch.tap(KeyModifier.Super, nowMs = 0)
        latch.lock(KeyModifier.Shift)
        latch.tap(KeyModifier.Ctrl, nowMs = 0)
        assertEquals(listOf(KeyModifier.Ctrl, KeyModifier.Shift, KeyModifier.Super), latch.consume())
        assertEquals("only the lock survives", listOf(KeyModifier.Shift), latch.active())
    }

    @Test
    fun resetClearsLocksToo() {
        val latch = ModifierLatch()
        var changes = 0
        latch.onChange = { changes++ }
        latch.lock(KeyModifier.Ctrl)
        latch.reset()
        assertEquals(emptyList<KeyModifier>(), latch.active())
        assertEquals(2, changes)
    }
}
