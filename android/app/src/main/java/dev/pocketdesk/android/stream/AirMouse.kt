package dev.pocketdesk.android.stream

import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The phone as an air mouse (F4): its gyroscope's turn, while held, moves
 * the computer's pointer. Pointed like a remote (its top toward the
 * screen), turning left and right is a turn about the phone's z axis and
 * tilting up and down one about its x axis. Kept apart from Android so it
 * is tested; the pad feeds it each gyroscope event.
 */
class AirMouse(private val speed: Float) {
    private var carryX = 0f
    private var carryY = 0f

    /**
     * The pointer's move for a turn of [rateX] and [rateZ] rad/s held for
     * [dtSec], in whole pixels; the fractions carry to the next. A turn
     * slower than [DEAD_ZONE] is a hand holding still, and moves nothing.
     */
    fun step(rateX: Float, rateZ: Float, dtSec: Float): Pair<Int, Int> {
        if (dtSec <= 0f || dtSec > MAX_STEP_SEC) return 0 to 0
        val x = if (abs(rateZ) < DEAD_ZONE) 0f else -rateZ * dtSec * PX_PER_RAD * speed + carryX
        val y = if (abs(rateX) < DEAD_ZONE) 0f else -rateX * dtSec * PX_PER_RAD * speed + carryY
        val dx = x.roundToInt()
        val dy = y.roundToInt()
        carryX = if (abs(rateZ) < DEAD_ZONE) 0f else x - dx
        carryY = if (abs(rateX) < DEAD_ZONE) 0f else y - dy
        return dx to dy
    }

    /** Forget what carried: the pad was let go. */
    fun reset() {
        carryX = 0f
        carryY = 0f
    }

    companion object {
        /** rad/s under which the hand is taken as still. */
        const val DEAD_ZONE = 0.03f

        /** Pixels a radian of turn moves the pointer at speed 1. */
        const val PX_PER_RAD = 900f

        /** A gap between events longer than this is a pause, not a turn. */
        const val MAX_STEP_SEC = 0.2f

        /** A press shorter than this, moving under [TAP_PX], is a click. */
        const val TAP_MS = 250L
        const val TAP_PX = 40

        /** Moves wait this long into a press, so a tap's thumb does not move the pointer. */
        const val HOLD_MS = 120L
    }
}

/**
 * One press of the air pad (F4 review): its first [AirMouse.HOLD_MS] the
 * moves are held back, so the thumb's own press tilting the phone moves
 * nothing; past it they go, the held ones first. A short press that moved
 * little is a click, and its held moves are dropped.
 */
class AirPress(private val downMs: Long) {
    private var heldX = 0
    private var heldY = 0
    private var total = 0

    /** The move to send now for a move of [dx], [dy] at [nowMs], or 0 to 0 while held back. */
    fun move(dx: Int, dy: Int, nowMs: Long): Pair<Int, Int> {
        total += kotlin.math.abs(dx) + kotlin.math.abs(dy)
        if (nowMs - downMs < AirMouse.HOLD_MS) {
            heldX += dx
            heldY += dy
            return 0 to 0
        }
        val out = (heldX + dx) to (heldY + dy)
        heldX = 0
        heldY = 0
        return out
    }

    /** Let go at [nowMs]: whether the press was a click. */
    fun release(nowMs: Long): Boolean = nowMs - downMs < AirMouse.TAP_MS && total < AirMouse.TAP_PX
}
