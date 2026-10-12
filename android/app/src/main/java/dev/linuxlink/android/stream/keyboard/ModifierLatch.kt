package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent

/** The modifiers the keyboard bar can latch, in the order combos press them. */
enum class KeyModifier(val keyCode: Int) {
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
    private val states = Array(KeyModifier.entries.size) { LatchState.Off }
    private val lastTapMs = LongArray(KeyModifier.entries.size) { Long.MIN_VALUE / 2 }

    /** Called after any change, so the bar can redraw. */
    var onChange: (() -> Unit)? = null

    fun state(modifier: KeyModifier): LatchState = states[modifier.ordinal]

    /**
     * A tap at [nowMs]: off latches for one key at once (no waiting to see
     * whether a double tap follows), a second tap within [DOUBLE_TAP_MS]
     * locks, and any later tap turns it off.
     */
    fun tap(modifier: KeyModifier, nowMs: Long) {
        val index = modifier.ordinal
        val quick = nowMs - lastTapMs[index] <= DOUBLE_TAP_MS
        lastTapMs[index] = nowMs
        set(
            modifier,
            when (state(modifier)) {
                LatchState.Off -> LatchState.Once
                LatchState.Once -> if (quick) LatchState.Locked else LatchState.Off
                LatchState.Locked -> LatchState.Off
            },
        )
    }

    fun lock(modifier: KeyModifier) = set(modifier, LatchState.Locked)

    fun active(): List<KeyModifier> = KeyModifier.entries.filter { state(it) != LatchState.Off }

    /** The modifiers for the next key; one-shot latches are used up. */
    fun consume(): List<KeyModifier> {
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

    private fun set(modifier: KeyModifier, value: LatchState) {
        states[modifier.ordinal] = value
        onChange?.invoke()
    }

    companion object {
        const val DOUBLE_TAP_MS = 300L
    }
}
