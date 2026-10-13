package dev.pocketdesk.android.stream

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
 * - a one-finger tap is a left click, but it is held back for
 *   [dragWindowMs] to learn whether the next touch makes it a drag or a
 *   double click: the click goes out when that window ends ([expire]), or
 *   earlier when something else happens. Nothing is ever pressed during the
 *   wait, so a lost timer or a torn-down view cannot leave a button stuck;
 * - two and three fingers tapped (no travel, lifted within [tapMaxMs]) are
 *   right and middle clicks, and two or more fingers moving scroll;
 * - a tap, then a touch within [dragWindowMs] that moves, drags with the
 *   left button held until the fingers lift. That is one press and no
 *   click first, so the desktop does not see a double click (which would
 *   maximise a title bar or open a file). A second tap in the window is a
 *   real double click.
 *
 * Deltas are in the desktop's pixels: the caller divides out zoom and maps
 * view pixels to video pixels first. All times are one clock in ms. Not
 * thread-safe: call from one thread.
 */
class TrackpadGesture(
    private val slop: Float,
    private val sensitivity: Float = 1f,
    private val dragWindowMs: Long = 300,
    private val tapMaxMs: Long = 400,
    /** Finger travel (view px) per wheel notch; the caller derives it from dp. */
    private val pxPerNotch: Float = 1f,
) {
    private var startMs = 0L
    private var maxFingers = 0
    private var travelled = 0f
    private var fingerDown = false
    private var armed = false
    private var dragging = false
    private var pendingClickDeadline: Long? = null
    private var moveCarryX = 0f
    private var moveCarryY = 0f
    private var scrollCarryX = 0f
    private var scrollCarryY = 0f

    /**
     * The time a held-back one-finger click is due, or null. The caller
     * schedules [expire] for then; calling it early or late is harmless.
     */
    fun pendingDeadline(): Long? = pendingClickDeadline

    /** Send the waiting click if its time has come and no finger is down. */
    fun expire(nowMs: Long): List<PadOutput> {
        val deadline = pendingClickDeadline ?: return emptyList()
        if (fingerDown || nowMs < deadline) return emptyList()
        pendingClickDeadline = null
        return listOf(PadOutput.Click(LEFT))
    }

    /** The first finger touches. A click that waited too long goes out first. */
    fun down(timeMs: Long): List<PadOutput> {
        val late = pendingClickDeadline?.let { timeMs >= it } == true
        val out = if (late) flushPending() else emptyList()
        fingerDown = true
        startMs = timeMs
        maxFingers = 1
        travelled = 0f
        armed = pendingClickDeadline != null
        moveCarryX = 0f
        moveCarryY = 0f
        scrollCarryX = 0f
        scrollCarryY = 0f
        return out
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
            // Another gesture begins: a tap that was waiting is a tap after all.
            val out = flushPending().toMutableList()
            armed = false
            // Like the cursor, scrolling waits for the slop, so the jitter of a
            // second finger landing does not scroll before a right-click.
            // Natural scrolling: the content follows the fingers (finger down =
            // content down = the view scrolls up = a positive wheel; finger right =
            // content right = a negative horizontal wheel).
            scrollCarryX -= dx / pxPerNotch
            scrollCarryY += dy / pxPerNotch
            if (travelled <= slop) return out
            val sx = scrollCarryX.toInt()
            val sy = scrollCarryY.toInt()
            if (sx == 0 && sy == 0) return out
            scrollCarryX -= sx
            scrollCarryY -= sy
            out += PadOutput.Scroll(sx, sy)
            return out
        }
        if (travelled <= slop) return emptyList()
        val out = mutableListOf<PadOutput>()
        if (armed && !dragging) {
            dragging = true
            armed = false
            // The waiting tap becomes this drag's press; no click goes out.
            pendingClickDeadline = null
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
        fingerDown = false
        if (dragging) {
            dragging = false
            return listOf(PadOutput.Button(LEFT, false))
        }
        if (travelled > slop) {
            armed = false
            return flushPending()
        }
        return when {
            maxFingers <= 1 && armed -> {
                // Second tap inside the window: a real double click.
                armed = false
                pendingClickDeadline = null
                listOf(PadOutput.Click(LEFT), PadOutput.Click(LEFT))
            }
            maxFingers <= 1 -> {
                pendingClickDeadline = timeMs + dragWindowMs
                emptyList()
            }
            timeMs - startMs > tapMaxMs -> flushPending()
            maxFingers == 2 -> flushPending() + PadOutput.Click(RIGHT)
            else -> flushPending() + PadOutput.Click(MIDDLE)
        }
    }

    /**
     * The gesture was torn down (the view left composition, a mode switch,
     * a layout change). Releases a held button, and drops a waiting click:
     * a lost click is better than a stuck button.
     */
    fun cancel(): List<PadOutput> {
        val out = if (dragging) listOf(PadOutput.Button(LEFT, false)) else emptyList()
        dragging = false
        armed = false
        fingerDown = false
        pendingClickDeadline = null
        return out
    }

    private fun flushPending(): List<PadOutput> {
        if (pendingClickDeadline == null) return emptyList()
        pendingClickDeadline = null
        return listOf(PadOutput.Click(LEFT))
    }

    companion object {
        /** `RustCore.sendMouseClick` button codes. */
        const val LEFT = 0
        const val MIDDLE = 1
        const val RIGHT = 2
    }
}
