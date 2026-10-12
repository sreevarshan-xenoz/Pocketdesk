package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/** The modifiers the keyboard bar can latch, in the order combos press them. */
enum class Modifier(val keyCode: Int) {
    Ctrl(KeyEvent.KEYCODE_CTRL_LEFT),
    Alt(KeyEvent.KEYCODE_ALT_LEFT),
    Shift(KeyEvent.KEYCODE_SHIFT_LEFT),
    Super(KeyEvent.KEYCODE_META_LEFT),
}

enum class LatchState { Off, Once, Locked }

/**
 * The bar's modifier keys. They latch on the phone only: the desktop never
 * sees a held modifier, because each key that uses them goes out as one
 * press-tap-release combo, so none can stick (spec K5). A tap latches for
 * the next key, a second tap unlatches, and [lock] (a double tap) keeps it
 * until tapped off.
 */
class ModifierLatch {
    private val states = Array(Modifier.entries.size) { LatchState.Off }

    /** Called after any change, so the bar can redraw. */
    var onChange: (() -> Unit)? = null

    fun state(modifier: Modifier): LatchState = states[modifier.ordinal]

    fun tap(modifier: Modifier) {
        set(modifier, if (state(modifier) == LatchState.Off) LatchState.Once else LatchState.Off)
    }

    fun lock(modifier: Modifier) = set(modifier, LatchState.Locked)

    fun active(): List<Modifier> = Modifier.entries.filter { state(it) != LatchState.Off }

    /** The modifiers for the next key; one-shot latches are used up. */
    fun consume(): List<Modifier> {
        val active = active()
        if (active.any { state(it) == LatchState.Once }) {
            active.filter { state(it) == LatchState.Once }.forEach { states[it.ordinal] = LatchState.Off }
            onChange?.invoke()
        }
        return active
    }

    fun reset() {
        if (active().isEmpty()) return
        states.fill(LatchState.Off)
        onChange?.invoke()
    }

    private fun set(modifier: Modifier, value: LatchState) {
        states[modifier.ordinal] = value
        onChange?.invoke()
    }
}
