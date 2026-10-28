package dev.pocketdesk.android.control

/**
 * The decisions of control from the computer (C8), kept apart from Android
 * so they are tested: where a point lands, how touches become gestures,
 * how text is edited, and how much is taken.
 */
object ControlPlan {
    /** A point on the phone's screen, in pixels. */
    data class Point(val x: Float, val y: Float)

    /** The top of a coordinate fraction on the wire. */
    const val FRACTION_MAX = 65535

    /**
     * The pixel a fraction ([x], [y] out of [FRACTION_MAX]) of the picture
     * lands on, on a screen [width]×[height] as it is drawn now: the cast
     * mirrors the whole screen, so the picture is the screen.
     */
    fun place(x: Int, y: Int, width: Int, height: Int): Point {
        val fx = x.coerceIn(0, FRACTION_MAX) / FRACTION_MAX.toFloat()
        val fy = y.coerceIn(0, FRACTION_MAX) / FRACTION_MAX.toFloat()
        return Point(fx * (width - 1).coerceAtLeast(0), fy * (height - 1).coerceAtLeast(0))
    }

    /** One stroke of a gesture to perform. */
    data class Segment(
        /** The path, from where the last segment ended (or the press). */
        val points: List<Point>,
        val durationMs: Long,
        /** The first of its gesture: a new stroke, not a continued one. */
        val start: Boolean,
        /** The finger stays down after it. */
        val willContinue: Boolean,
    )

    /** The shortest stroke; a tap is at least [TAP_MS]. */
    const val MIN_SEGMENT_MS = 10L
    const val TAP_MS = 40L
    /** The longest a segment takes: a late batch is not replayed slowly. */
    const val MAX_SEGMENT_MS = 300L
    /** A press with no move or release yet starts its stroke after this, so a long press holds. */
    const val START_WAIT_MS = 80L
    /** A finger left down this long with no event is lifted. */
    const val IDLE_LIFT_MS = 2_000L
    /** Most moves waiting for the next segment. */
    const val MAX_PENDING = 256

    /**
     * Touches from the computer as gestures. The phone performs one segment
     * at a time ([next] when none is being performed); moves that arrive
     * meanwhile join the next segment, so the finger follows the pointer
     * without queueing up behind it. A press and release with nothing
     * between is one tap.
     */
    class Strokes {
        private var down: Point? = null
        private var downAt = 0L
        private var started = false
        private var last: Point? = null
        private var lastAt = 0L
        private var lastEvent = 0L
        private val pending = mutableListOf<Point>()
        private var up = false

        /** A finger is down, or about to be. */
        val active: Boolean get() = down != null

        fun down(point: Point, nowMs: Long) {
            reset()
            down = point
            downAt = nowMs
            lastEvent = nowMs
        }

        fun move(point: Point, nowMs: Long) {
            if (down == null || up) return
            // Bounded while a segment is held up: the newest point replaces
            // the last, so the finger still ends where the pointer is.
            if (pending.size >= MAX_PENDING) pending.removeAt(pending.size - 1)
            pending += point
            lastEvent = nowMs
        }

        fun up(point: Point, nowMs: Long) {
            if (down == null || up) return
            pending += point
            up = true
            lastEvent = nowMs
        }

        /** Lift a finger that is down now, wherever it is (the cast ended, control went off). */
        fun lift(nowMs: Long) {
            if (down != null && !up) up(pending.lastOrNull() ?: last ?: down!!, nowMs)
        }

        /**
         * BK5: the computer is still there with the button held (its pings
         * come): a finger down and not let go of is not idle.
         */
        fun keep(nowMs: Long) {
            if (down != null && !up) lastEvent = nowMs
        }

        /** Lift a finger left down with nothing from the computer for [IDLE_LIFT_MS]. */
        fun liftIfIdle(nowMs: Long) {
            if (down != null && !up && nowMs - lastEvent >= IDLE_LIFT_MS) {
                up(pending.lastOrNull() ?: last ?: down!!, nowMs)
            }
        }

        /** Whether [next] may have something at [nowMs] (a press waiting to start counts). */
        fun waiting(nowMs: Long): Boolean =
            down != null && (pending.isNotEmpty() || (!started && nowMs - downAt >= START_WAIT_MS))

        /** The next segment to perform at [nowMs], or null. Call only when none is being performed. */
        fun next(nowMs: Long): Segment? {
            val press = down ?: return null
            if (!started) {
                if (pending.isEmpty()) {
                    if (nowMs - downAt < START_WAIT_MS) return null
                    // Held still: the finger goes down, and stays.
                    started = true
                    last = press
                    lastAt = nowMs
                    return Segment(listOf(press), START_WAIT_MS, start = true, willContinue = true)
                }
                started = true
                val points = listOf(press) + pending
                pending.clear()
                val done = up
                val floor = if (done) TAP_MS else MIN_SEGMENT_MS
                val duration = (nowMs - downAt).coerceIn(floor, MAX_SEGMENT_MS)
                finish(points.last(), nowMs, done)
                return Segment(points, duration, start = true, willContinue = !done)
            }
            if (pending.isEmpty()) return null
            val from = last ?: press
            val points = listOf(from) + pending
            pending.clear()
            val done = up
            val duration = (nowMs - lastAt).coerceIn(MIN_SEGMENT_MS, MAX_SEGMENT_MS)
            finish(points.last(), nowMs, done)
            return Segment(points, duration, start = false, willContinue = !done)
        }

        /** The gesture was cancelled by Android: start over with the next press. */
        fun cancelled() = reset()

        private fun finish(end: Point, nowMs: Long, done: Boolean) {
            last = end
            lastAt = nowMs
            if (done) reset()
        }

        private fun reset() {
            down = null
            started = false
            last = null
            pending.clear()
            up = false
        }
    }

    /** A field's text after an edit, and where the cursor goes. */
    data class Edited(val text: String, val cursor: Int)

    /** [inserted] typed into [text] at its selection [start]..[end] (-1 for none: the end). */
    fun type(text: String, start: Int, end: Int, inserted: String): Edited {
        val (from, to) = selection(text, start, end)
        return Edited(text.substring(0, from) + inserted + text.substring(to), from + inserted.length)
    }

    /** Backspace in [text] at its selection: the selection, else the character before the cursor. */
    fun backspace(text: String, start: Int, end: Int): Edited {
        val (from, to) = selection(text, start, end)
        if (from != to) return Edited(text.substring(0, from) + text.substring(to), from)
        if (from == 0) return Edited(text, 0)
        // A whole character: a surrogate pair is one.
        val before = Character.offsetByCodePoints(text, from, -1)
        return Edited(text.substring(0, before) + text.substring(from), before)
    }

    private fun selection(text: String, start: Int, end: Int): Pair<Int, Int> {
        if (start < 0 || end < 0) return text.length to text.length
        val a = start.coerceIn(0, text.length)
        val b = end.coerceIn(0, text.length)
        return minOf(a, b) to maxOf(a, b)
    }

    /** At most this many events a second are taken. */
    const val MAX_EVENTS_PER_SECOND = 200

    /** Counts events per second; more than [MAX_EVENTS_PER_SECOND] are refused. */
    class Rate {
        private var second = -1L
        private var count = 0

        fun take(nowMs: Long): Boolean {
            val s = nowMs / 1000
            if (s != second) {
                second = s
                count = 0
            }
            if (count >= MAX_EVENTS_PER_SECOND) return false
            count++
            return true
        }
    }

    /**
     * A wheel's [notches] as a swipe at [at] on a screen [height] tall: the
     * finger moves against the content, a tenth of the screen a notch, at
     * most half of it. Positive [notches] scroll down (the finger goes up).
     */
    fun scroll(at: Point, notches: Int, height: Int): List<Point> {
        val step = height / 10f
        val travel = (notches * step).coerceIn(-height / 2f, height / 2f)
        val endY = (at.y - travel).coerceIn(0f, (height - 1).coerceAtLeast(0).toFloat())
        return listOf(at, Point(at.x, endY))
    }
}
