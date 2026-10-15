package dev.pocketdesk.android.stream

/**
 * A mouse connected to the phone, captured by the session (T9c): Android's
 * captured pointer gives raw relative motion, the button state and the wheel;
 * this turns them into the trackpad's outputs, so the same sender puts them
 * on the wire. Kept apart from Android so it is tested. Not thread-safe.
 */
class CapturedMouse(private val sensitivity: Float = 1f) {
    private var held = 0
    private var carryX = 0f
    private var carryY = 0f
    private var scrollX = 0f
    private var scrollY = 0f

    /**
     * One captured event: motion ([dx], [dy], device units), the buttons
     * held now (Android's `MotionEvent.getButtonState()` bits), and the
     * wheel ([vscroll] up positive, [hscroll] right positive, in notches).
     */
    fun event(
        dx: Float,
        dy: Float,
        buttons: Int,
        vscroll: Float = 0f,
        hscroll: Float = 0f,
    ): List<PadOutput> {
        val out = mutableListOf<PadOutput>()
        carryX += dx * sensitivity
        carryY += dy * sensitivity
        val mx = carryX.toInt()
        val my = carryY.toInt()
        if (mx != 0 || my != 0) {
            carryX -= mx
            carryY -= my
            out += PadOutput.Move(mx, my)
        }
        out += buttonChanges(buttons)
        scrollX += hscroll
        scrollY += vscroll
        val sx = scrollX.toInt()
        val sy = scrollY.toInt()
        if (sx != 0 || sy != 0) {
            scrollX -= sx
            scrollY -= sy
            out += PadOutput.Scroll(sx, sy)
        }
        return out
    }

    /** The capture ended: release every button still held. */
    fun release(): List<PadOutput> = buttonChanges(0)

    /** Releases, then presses, for the buttons that changed to [now]. */
    private fun buttonChanges(now: Int): List<PadOutput> {
        val out = mutableListOf<PadOutput>()
        for ((bit, wire) in BUTTONS) {
            if (held and bit != 0 && now and bit == 0) out += PadOutput.Button(wire, false)
        }
        for ((bit, wire) in BUTTONS) {
            if (held and bit == 0 && now and bit != 0) out += PadOutput.Button(wire, true)
        }
        held = now
        return out
    }

    companion object {
        /** Whether a session holds the pointer now (read by MainActivity's key dispatch). */
        @Volatile
        var captured = false

        /** One axis of a batched event: Android keeps a fast mouse's earlier samples as history. */
        fun batched(history: List<Float>, current: Float): Float = history.sum() + current

        /**
         * Whether a key event is swallowed: Android turns a mouse's Back and
         * Forward buttons into the phone's Back and Forward keys too, and
         * while the pointer is captured they belong to the desktop (a Back
         * would end the session).
         */
        fun swallowsKey(keyCode: Int, fromMouse: Boolean, captured: Boolean): Boolean =
            captured && fromMouse && (keyCode == KEYCODE_BACK || keyCode == KEYCODE_FORWARD)

        private const val KEYCODE_BACK = 4
        private const val KEYCODE_FORWARD = 125

        /** Android's button bits, and the wire's button codes. */
        private val BUTTONS = listOf(
            1 to TrackpadGesture.LEFT, // BUTTON_PRIMARY
            2 to TrackpadGesture.RIGHT, // BUTTON_SECONDARY
            4 to TrackpadGesture.MIDDLE, // BUTTON_TERTIARY
            8 to 3, // BUTTON_BACK
            16 to 4, // BUTTON_FORWARD
        )
    }
}
