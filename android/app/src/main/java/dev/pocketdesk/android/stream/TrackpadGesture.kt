package dev.pocketdesk.android.stream

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.roundToInt

/** What a trackpad gesture sends. Buttons use the wire encoding (see [TrackpadGesture]). */
sealed interface PadOutput {
    data class Move(val dx: Int, val dy: Int) : PadOutput

    /**
     * Wheel steps, content following the fingers. With [fine] (smooth
     * scrolling) the turn also goes in 120ths of a notch ([x120], [y120]),
     * and [dx]/[dy] are the whole notches it completes, for a computer older
     * than fine scrolling; such an output may complete none.
     */
    data class Scroll(
        val dx: Int,
        val dy: Int,
        val x120: Int = 0,
        val y120: Int = 0,
        val fine: Boolean = false,
    ) : PadOutput

    data class Click(val button: Int) : PadOutput

    data class Button(val button: Int, val pressed: Boolean) : PadOutput

    /**
     * Zoom the view by [factor] (T9a): the caller applies it, and pans by
     * the fingers' centre. Nothing goes to the desktop.
     */
    data class Zoom(val factor: Float) : PadOutput
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
 * - two fingers spreading or pinching zoom the view (T9a): a two-finger
 *   gesture whose span changes past [slop] before its centre travels as far
 *   (counted from when the second finger landed) is a pinch, otherwise a
 *   scroll, and it stays what it began as until the fingers lift. A pinch is
 *   never a click; each step zooms by at most 2x either way, and a pair that
 *   breaks (a finger lifts) starts its span afresh. Only exactly two fingers
 *   pinch, and only for a caller that gives the span (the dock does not);
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
    /**
     * Smooth scrolling: two-finger scroll also goes in 120ths of a notch,
     * every move, not only when a notch is complete.
     */
    private val fine: Boolean = false,
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
    private var fineCarryX = 0f
    private var fineCarryY = 0f
    private var twoFinger = TwoFinger.UNDECIDED
    private var twoTravel = 0f
    private var startSpan = 0f
    private var lastSpan = 0f

    private enum class TwoFinger { UNDECIDED, SCROLL, PINCH }

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
        fineCarryX = 0f
        fineCarryY = 0f
        twoFinger = TwoFinger.UNDECIDED
        twoTravel = 0f
        startSpan = 0f
        lastSpan = 0f
        return out
    }

    /**
     * The fingers moved by ([dx], [dy]) with [fingers] down. Cursor motion is
     * further scaled by [cursorScale] (view to video pixels); scrolling and
     * the slop are not. With two or more fingers, ([dx], [dy]) is their
     * centre's motion. [span] is the distance between two fingers (view px),
     * or 0 when the caller does not zoom or the pair just changed.
     */
    fun move(dx: Float, dy: Float, fingers: Int, cursorScale: Float = 1f, span: Float = 0f): List<PadOutput> {
        maxFingers = maxOf(maxFingers, fingers)
        travelled += hypot(dx, dy)
        if (maxFingers >= 2 && !dragging) {
            // Another gesture begins: a tap that was waiting is a tap after all.
            val out = flushPending().toMutableList()
            armed = false
            // Only the motion since the second finger landed decides.
            twoTravel += hypot(dx, dy)
            val pairSpan = if (fingers == 2) span else 0f
            if (pairSpan > 0f && startSpan == 0f) startSpan = pairSpan
            if (twoFinger == TwoFinger.UNDECIDED) {
                val spread = if (startSpan > 0f && pairSpan > 0f) abs(pairSpan - startSpan) else 0f
                if (spread > slop && spread > twoTravel) {
                    twoFinger = TwoFinger.PINCH
                    // The spread so far is zoomed now, from where it began.
                    lastSpan = startSpan
                } else if (twoTravel > slop) {
                    twoFinger = TwoFinger.SCROLL
                }
            }
            if (twoFinger == TwoFinger.PINCH) {
                when {
                    // The pair broke (a finger lifted): its span starts afresh.
                    pairSpan <= 0f -> lastSpan = 0f
                    lastSpan <= 0f -> lastSpan = pairSpan
                    else -> {
                        out += PadOutput.Zoom((pairSpan / lastSpan).coerceIn(MIN_ZOOM_STEP, MAX_ZOOM_STEP))
                        lastSpan = pairSpan
                    }
                }
                return out
            }
            // Like the cursor, scrolling waits for the slop, so the jitter of a
            // second finger landing does not scroll before a right-click.
            // Natural scrolling: the content follows the fingers (finger down =
            // content down = the view scrolls up = a positive wheel; finger right =
            // content right = a negative horizontal wheel).
            scrollCarryX -= dx / pxPerNotch
            scrollCarryY += dy / pxPerNotch
            if (fine) {
                fineCarryX -= dx / pxPerNotch * NOTCH
                fineCarryY += dy / pxPerNotch * NOTCH
            }
            if (twoFinger != TwoFinger.SCROLL) return out
            val sx = scrollCarryX.toInt()
            val sy = scrollCarryY.toInt()
            scrollCarryX -= sx
            scrollCarryY -= sy
            if (fine) {
                val fx = fineCarryX.toInt()
                val fy = fineCarryY.toInt()
                if (fx == 0 && fy == 0 && sx == 0 && sy == 0) return out
                fineCarryX -= fx
                fineCarryY -= fy
                out += PadOutput.Scroll(sx, sy, fx, fy, fine = true)
                return out
            }
            if (sx == 0 && sy == 0) return out
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
        if (travelled > slop || twoFinger == TwoFinger.PINCH) {
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
        /** One wheel notch in the fine units (`wheel120`). */
        const val NOTCH = 120f

        /** `RustCore.sendMouseClick` button codes. */
        const val LEFT = 0
        const val MIDDLE = 1
        const val RIGHT = 2

        /** The most one step of a pinch zooms out, and in. */
        const val MIN_ZOOM_STEP = 0.5f
        const val MAX_ZOOM_STEP = 2f
    }
}
