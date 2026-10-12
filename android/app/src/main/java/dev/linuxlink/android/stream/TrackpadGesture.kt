package dev.linuxlink.android.stream

import kotlin.math.hypot
import kotlin.math.roundToInt

/** What a trackpad gesture sends. Buttons use the wire encoding (see [TrackpadGesture]). */
sealed interface PadOutput {
    data class Move(val dx: Int, val dy: Int) : PadOutput

    /** Wheel steps, content following the fingers. */
    data class Scroll(val dx: Int, val dy: Int) : PadOutput

    data class Click(val button: Int) : PadOutput

    data class Button(val button: Int, val pressed: Boolean) : PadOutput
}

/**
 * The trackpad's gestures, laptop style (roadmap F3), with no Compose in
 * them so they can be tested. Both trackpads, the in-stream one and the
 * foldable dock, feed pointer events in and send what comes out:
 *
 * - one finger moves the cursor once it passes [slop], scaled by
 *   [sensitivity], with the sub-pixel remainder carried;
 * - a one-finger tap is a left click; two and three fingers tapped (no
 *   travel, and lifted within [tapMaxMs]) are right and middle clicks;
 * - two or more fingers moving scroll;
 * - a tap, then a touch within [dragWindowMs] that moves, drags with the
 *   left button held until the fingers lift, and a second tap in that
 *   window is a double click.
 *
 * Deltas are in the desktop's pixels: the caller divides out zoom and maps
 * view pixels to video pixels first.
 */
class TrackpadGesture(
    private val slop: Float,
    private val sensitivity: Float = 1f,
    private val dragWindowMs: Long = 300,
    private val tapMaxMs: Long = 400,
) {
    private var startMs = 0L
    private var maxFingers = 0
    private var travelled = 0f
    private var armed = false
    private var dragging = false
    private var lastTapUpMs = Long.MIN_VALUE / 2
    private var moveCarryX = 0f
    private var moveCarryY = 0f
    private var scrollCarryX = 0f
    private var scrollCarryY = 0f

    /** The first finger touches. */
    fun down(timeMs: Long) {
        startMs = timeMs
        maxFingers = 1
        travelled = 0f
        armed = timeMs - lastTapUpMs <= dragWindowMs
        moveCarryX = 0f
        moveCarryY = 0f
        scrollCarryX = 0f
        scrollCarryY = 0f
    }

    /**
     * The fingers moved by ([dx], [dy]) with [fingers] down. Cursor motion is
     * further scaled by [cursorScale] (view to video pixels); scrolling and
     * the slop are not.
     */
    fun move(dx: Float, dy: Float, fingers: Int, cursorScale: Float = 1f): List<PadOutput> {
        maxFingers = maxOf(maxFingers, fingers)
        travelled += hypot(dx, dy)
        if (maxFingers >= 2 && !dragging) {
            scrollCarryX += dx
            scrollCarryY += dy
            val sx = scrollCarryX.roundToInt()
            // Finger down == content down == negative wheel.
            val sy = (-scrollCarryY).roundToInt()
            if (sx == 0 && sy == 0) return emptyList()
            scrollCarryX -= sx
            scrollCarryY += sy
            return listOf(PadOutput.Scroll(sx, sy))
        }
        if (travelled <= slop) return emptyList()
        val out = mutableListOf<PadOutput>()
        if (armed && !dragging) {
            dragging = true
            out += PadOutput.Button(LEFT, true)
        }
        moveCarryX += dx * sensitivity * cursorScale
        moveCarryY += dy * sensitivity * cursorScale
        val mx = moveCarryX.toInt()
        val my = moveCarryY.toInt()
        if (mx != 0 || my != 0) {
            moveCarryX -= mx
            moveCarryY -= my
            out += PadOutput.Move(mx, my)
        }
        return out
    }

    /** The last finger lifted. */
    fun up(timeMs: Long): List<PadOutput> {
        if (dragging) {
            dragging = false
            lastTapUpMs = Long.MIN_VALUE / 2
            return listOf(PadOutput.Button(LEFT, false))
        }
        if (travelled > slop) return emptyList()
        return when {
            maxFingers <= 1 -> {
                lastTapUpMs = timeMs
                listOf(PadOutput.Click(LEFT))
            }
            timeMs - startMs > tapMaxMs -> emptyList()
            maxFingers == 2 -> listOf(PadOutput.Click(RIGHT))
            else -> listOf(PadOutput.Click(MIDDLE))
        }
    }

    companion object {
        /** `RustCore.sendMouseClick` button codes. */
        const val LEFT = 0
        const val MIDDLE = 1
        const val RIGHT = 2
    }
}
