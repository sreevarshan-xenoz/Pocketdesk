package dev.pocketdesk.android.stream

import dev.pocketdesk.android.bridge.RustCore
import kotlin.math.PI
import kotlin.math.atan
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.tan

/**
 * How hard a touch must press for full ink (E3): Soft gives more for a
 * light touch, Firm needs more force. A curve on the 0..1 pressure Android
 * reports, so Even is the phone's own reading.
 */
enum class PenCurve(val key: String, private val gamma: Float) {
    Soft("soft", 0.6f),
    Even("even", 1f),
    Firm("firm", 1.6f),
    ;

    /** [raw] (Android's 0..1, a finger's often above 1) as 0..=65535. */
    fun apply(raw: Float): Int {
        val p = if (raw.isNaN()) 0f else raw.coerceIn(0f, 1f)
        return (p.pow(gamma) * MAX_PRESSURE).roundToInt().coerceIn(0, MAX_PRESSURE)
    }

    companion object {
        const val MAX_PRESSURE = 65535

        fun fromKey(key: String?): PenCurve = entries.firstOrNull { it.key == key } ?: Even
    }
}

/** How long a stylus's hover end waits for its touch (E3 review). */
const val EXIT_GRACE_MS = 80L

/** The input modes a session offers: the pen only where the computer takes one (E3). */
fun inputModes(pen: Boolean): List<InputMode> =
    if (pen) {
        listOf(InputMode.DirectTouch, InputMode.Trackpad, InputMode.Pen)
    } else {
        listOf(InputMode.DirectTouch, InputMode.Trackpad)
    }

/** The mode chip's next mode in [modes]; one no longer offered goes back to the first. */
fun nextMode(mode: InputMode, modes: List<InputMode>): InputMode {
    val at = modes.indexOf(mode)
    return if (at < 0) modes.first() else modes[(at + 1) % modes.size]
}

/** One pen sample as the wire says it (core's `PenSample`). */
data class PenSample(
    val xNorm: Int,
    val yNorm: Int,
    val pressure: Int,
    val tiltX: Int,
    val tiltY: Int,
    val flags: Int,
) {
    val inRange: Boolean get() = flags and (RustCore.PEN_IN_RANGE or RustCore.PEN_CONTACT) != 0
}

/**
 * A stylus's tilt as the pen's two angles (E3). Android gives the angle from
 * upright ([tilt], 0..pi/2) and the direction it leans ([orientation], 0 is
 * toward the top of the screen); the computer takes how far it leans along
 * each axis, in degrees, as Chromium works them out.
 */
fun penTilt(tilt: Float, orientation: Float): Pair<Int, Int> {
    if (tilt.isNaN() || orientation.isNaN() || tilt <= 0f) return 0 to 0
    val lean = tan(tilt.coerceAtMost((PI / 2 - 1e-3).toFloat()).toDouble())
    val toDegrees = 180.0 / PI
    val x = atan(sin(-orientation.toDouble()) * lean) * toDegrees
    val y = atan(cos(-orientation.toDouble()) * lean) * toDegrees
    return x.roundToInt().coerceIn(-90, 90) to y.roundToInt().coerceIn(-90, 90)
}

/**
 * The phone's touch as a pen (E3), event by event: what to send for each,
 * or nothing. A finger touching is a pen in contact; a stylus above the
 * screen is a pen hovering; a second finger is a pinch, so the pen lifts
 * first and stays lifted until every finger is up.
 */
class PenStroke(private val curve: PenCurve) {
    private var last: PenSample? = null
    private var pinching = false

    /**
     * A stylus's hover ended: Android says so just before it touches, so the
     * leaving waits [EXIT_GRACE_MS] for a touch ([expireExit]) rather than
     * dropping out of range before every stroke (E3 review).
     */
    private var exitPending = false

    /**
     * One event at [at] (normalized): [contact] while touching, [hovering]
     * for a stylus in range above it, [fingers] pressed now, [exiting] when a
     * stylus's hover ended. Pressure and tilt as Android reports them.
     */
    fun event(
        at: Pair<Int, Int>,
        contact: Boolean,
        hovering: Boolean,
        fingers: Int,
        pressure: Float,
        tilt: Float = 0f,
        orientation: Float = 0f,
        barrel: Boolean = false,
        eraser: Boolean = false,
        exiting: Boolean = false,
    ): PenSample? {
        if (exiting && !contact && last?.inRange == true) {
            exitPending = true
            return null
        }
        exitPending = false
        if (fingers >= 2) {
            pinching = true
            return lift()
        }
        if (pinching) {
            if (fingers == 0) pinching = false
            return null
        }
        val inRange = contact || hovering
        if (!inRange && last?.inRange != true) return null
        var flags = 0
        if (contact) flags = flags or RustCore.PEN_CONTACT
        if (inRange) flags = flags or RustCore.PEN_IN_RANGE
        if (inRange && barrel) flags = flags or RustCore.PEN_BARREL
        if (inRange && eraser) flags = flags or RustCore.PEN_ERASER
        val (tiltX, tiltY) = penTilt(tilt, orientation)
        val sample = PenSample(
            xNorm = at.first,
            yNorm = at.second,
            pressure = if (contact) curve.apply(pressure) else 0,
            tiltX = tiltX,
            tiltY = tiltY,
            flags = flags,
        )
        last = sample
        return sample
    }

    /** A hover's end with no touch after it: the pen leaves now. */
    fun expireExit(): PenSample? {
        if (!exitPending) return null
        exitPending = false
        return lift()
    }

    /** The pen leaves where it was, if it is in range (a pinch, the view gone). */
    fun lift(): PenSample? {
        val was = last?.takeIf { it.inRange } ?: return null
        val gone = was.copy(pressure = 0, flags = 0)
        last = gone
        return gone
    }
}
