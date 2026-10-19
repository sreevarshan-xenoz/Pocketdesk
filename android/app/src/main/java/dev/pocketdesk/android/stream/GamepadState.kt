package dev.pocketdesk.android.stream

import android.view.KeyEvent
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * The gamepad packet's button bits (input tag 5; the computer's
 * `GAMEPAD_BUTTONS`). Never renumbered.
 */
object PadButton {
    const val A = 0x0001
    const val B = 0x0002
    const val X = 0x0004
    const val Y = 0x0008
    const val LB = 0x0010
    const val RB = 0x0020
    const val SELECT = 0x0040
    const val START = 0x0080
    const val HOME = 0x0100
    const val LEFT_STICK = 0x0200
    const val RIGHT_STICK = 0x0400
    const val UP = 0x0800
    const val DOWN = 0x1000
    const val LEFT = 0x2000
    const val RIGHT = 0x4000

    private const val DPAD = UP or DOWN or LEFT or RIGHT

    /** The D-pad's bits for a hat at (x, y), each -1..1, y down. */
    fun hat(x: Float, y: Float): Int {
        var bits = 0
        if (x <= -0.5f) bits = bits or LEFT
        if (x >= 0.5f) bits = bits or RIGHT
        if (y <= -0.5f) bits = bits or UP
        if (y >= 0.5f) bits = bits or DOWN
        return bits
    }

    /** The bit a controller's key is, or 0 when it is not a gamepad key. */
    fun ofKey(keyCode: Int): Int = when (keyCode) {
        KeyEvent.KEYCODE_BUTTON_A -> A
        KeyEvent.KEYCODE_BUTTON_B -> B
        KeyEvent.KEYCODE_BUTTON_X -> X
        KeyEvent.KEYCODE_BUTTON_Y -> Y
        KeyEvent.KEYCODE_BUTTON_L1 -> LB
        KeyEvent.KEYCODE_BUTTON_R1 -> RB
        KeyEvent.KEYCODE_BUTTON_SELECT -> SELECT
        KeyEvent.KEYCODE_BUTTON_START -> START
        KeyEvent.KEYCODE_BUTTON_MODE -> HOME
        KeyEvent.KEYCODE_BUTTON_THUMBL -> LEFT_STICK
        KeyEvent.KEYCODE_BUTTON_THUMBR -> RIGHT_STICK
        KeyEvent.KEYCODE_DPAD_UP -> UP
        KeyEvent.KEYCODE_DPAD_DOWN -> DOWN
        KeyEvent.KEYCODE_DPAD_LEFT -> LEFT
        KeyEvent.KEYCODE_DPAD_RIGHT -> RIGHT
        else -> 0
    }

    internal fun dpad(bits: Int): Int = bits and DPAD
}

/**
 * The phone's gamepad (E4), on screen or connected: the buttons held and
 * the sticks, as the packet carries them. Each change says whether there is
 * anything new to send; [moving] says whether to keep sending, as the
 * computer moves the mouse and scrolls once per packet.
 */
class GamepadState {
    var buttons = 0
        private set

    /** Left stick x, y; right stick x, y; the triggers. ±32767, y down. */
    val axes = IntArray(6)

    /** A stick is off its centre: the packet is sent again and again. */
    val moving: Boolean get() = (0 until 4).any { axes[it] != 0 }

    fun press(bit: Int, down: Boolean): Boolean {
        val next = if (down) buttons or bit else buttons and bit.inv()
        return replaceButtons(next)
    }

    /** The left ([left]) or right stick at (x, y), each -1..1, y down. */
    fun stick(left: Boolean, x: Float, y: Float): Boolean {
        val (ax, ay) = stickAxes(x, y)
        val at = if (left) 0 else 2
        if (axes[at] == ax && axes[at + 1] == ay) return false
        axes[at] = ax
        axes[at + 1] = ay
        return true
    }

    /** A hat (a controller's D-pad as an axis) at (x, y). */
    fun hat(x: Float, y: Float): Boolean =
        replaceButtons(buttons and PadButton.dpad(buttons).inv() or PadButton.hat(x, y))

    /** Everything up and centred; whether anything was held. */
    fun reset(): Boolean {
        val held = buttons != 0 || axes.any { it != 0 }
        buttons = 0
        axes.fill(0)
        return held
    }

    private fun replaceButtons(next: Int): Boolean {
        if (next == buttons) return false
        buttons = next
        return true
    }

    companion object {
        /** A stick within this of its centre is centred. */
        const val DEAD_ZONE = 0.15f
        const val AXIS_MAX = 32767

        /** (x, y), each -1..1, as the packet's axes: centred within [DEAD_ZONE], at most the edge. */
        fun stickAxes(x: Float, y: Float): Pair<Int, Int> {
            val length = hypot(x, y)
            if (length.isNaN() || length < DEAD_ZONE) return 0 to 0
            // Past the dead zone the stick starts from nothing, and a
            // diagonal at the rim is held to the rim.
            val scale = ((length - DEAD_ZONE) / (1f - DEAD_ZONE)).coerceAtMost(1f) / length
            fun axis(v: Float) = (v * scale * AXIS_MAX).roundToInt().coerceIn(-AXIS_MAX, AXIS_MAX)
            return axis(x) to axis(y)
        }

        /** The time between packets while a stick is off centre. */
        const val REPEAT_MS = 33L
    }
}
