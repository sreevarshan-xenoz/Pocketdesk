package dev.pocketdesk.android.stream

import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent

/**
 * A game controller connected to the phone (E4), handed to the session in
 * front while it takes one: its buttons and D-pad as keys, its sticks and hat
 * as motion. The activity offers it every key and generic motion first.
 */
object ConnectedGamepad {
    /** What the session does with a controller. */
    interface Sink {
        fun button(bit: Int, down: Boolean)

        /** Left stick, right stick and hat, each -1..1, y down. */
        fun motion(lx: Float, ly: Float, rx: Float, ry: Float, hatX: Float, hatY: Float)
    }

    @Volatile
    var sink: Sink? = null

    private fun fromController(source: Int) =
        source and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
            source and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK

    /** Whether the session took [event]; a keyboard's arrows are not taken. */
    fun key(event: KeyEvent): Boolean {
        val sink = sink ?: return false
        if (!fromController(event.source)) return false
        val bit = PadButton.ofKey(event.keyCode)
        if (bit == 0) return false
        // Held keys repeat: the first press is the press.
        if (event.repeatCount == 0) sink.button(bit, event.action == KeyEvent.ACTION_DOWN)
        return true
    }

    /** Whether the session took a controller's sticks and hat. */
    fun motion(event: MotionEvent): Boolean {
        val sink = sink ?: return false
        if (!fromController(event.source) || event.actionMasked != MotionEvent.ACTION_MOVE) return false
        sink.motion(
            event.getAxisValue(MotionEvent.AXIS_X),
            event.getAxisValue(MotionEvent.AXIS_Y),
            event.getAxisValue(MotionEvent.AXIS_Z),
            event.getAxisValue(MotionEvent.AXIS_RZ),
            event.getAxisValue(MotionEvent.AXIS_HAT_X),
            event.getAxisValue(MotionEvent.AXIS_HAT_Y),
        )
        return true
    }
}
