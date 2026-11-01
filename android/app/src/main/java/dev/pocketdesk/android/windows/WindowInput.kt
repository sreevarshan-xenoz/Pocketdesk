package dev.pocketdesk.android.windows

import org.json.JSONObject

/**
 * Phone apps tier 2: how the computer's touch, wheel and typing in an app
 * window become input to that window's display. Pure, for JVM tests;
 * [WindowHostService] injects what this decides, with the display's id.
 *
 * Points arrive as fractions of the window's picture, 0 to 65535 on each
 * axis, so they land the same whatever size the window is made at.
 */
object WindowInput {
    /** The top of a point's fraction, as `core::streaming::cast_control`. */
    const val FRACTION_MAX = 65535

    /** Android's own numbers for what is injected (constants, so JVM tests need no framework). */
    const val ACTION_DOWN = 0
    const val ACTION_UP = 1
    const val ACTION_MOVE = 2
    const val ACTION_SCROLL = 8
    const val ACTION_POINTER_DOWN = 5
    const val ACTION_POINTER_UP = 6
    const val ACTION_MASK = 0xff
    const val ACTION_POINTER_INDEX_SHIFT = 8

    /** Most fingers at once, as `core::streaming::cast_control::MAX_POINTERS`. */
    const val MAX_POINTERS = 10
    const val KEYCODE_BACK = 4
    const val KEYCODE_ENTER = 66
    const val KEYCODE_DEL = 67

    /** The most notches one wheel event moves. */
    const val MAX_NOTCHES = 10

    /** The most text one event types, as the computer's own limit. */
    const val MAX_TEXT = 1024

    /** A pixel of a [size]-pixel side for a fraction: the first pixel at 0, the last at the top. */
    fun pixel(fraction: Int, size: Int): Float {
        if (size <= 1) return 0f
        return fraction.coerceIn(0, FRACTION_MAX).toFloat() / FRACTION_MAX * (size - 1)
    }

    /** The most events one batch is read for, as the computer's own batch limit. */
    const val MAX_EVENTS_PER_BATCH = 64

    /** The most batches a second are read in full; past it only releases go on (see [isRelease]). */
    const val MAX_BATCHES_PER_SECOND = 250

    /** The most fingers a second one window takes going down (a finger lifted is never limited). */
    const val MAX_DOWNS_PER_SECOND = 100

    /** Fingers down with no word from the computer for this long are lifted. */
    const val FINGER_SILENCE_MS = 2000L

    /**
     * Whether [event] lifts something: a finger up or a close. These go even when a batch is over
     * its limit: an up only does anything for a finger that is down (see [PointerTracker.limited]),
     * so it is bounded by the downs that were let through.
     */
    fun isRelease(event: Event): Boolean = when (event) {
        is Event.Touch -> event.action == ACTION_UP
        is Event.Pointer -> event.action == ACTION_UP
        Event.Close -> true
        else -> false
    }

    /** Whether fingers are down ([down]) and the computer has been silent since [lastMs] until [nowMs]. */
    fun silent(down: Boolean, lastMs: Long, nowMs: Long): Boolean = down && nowMs - lastMs >= FINGER_SILENCE_MS

    /** The motion action a touch event's word means, or null for one that is not. */
    fun motionAction(word: String): Int? = when (word) {
        "down" -> ACTION_DOWN
        "move" -> ACTION_MOVE
        "up" -> ACTION_UP
        else -> null
    }

    /**
     * The wheel's vertical amount for [notches] the computer sent (positive
     * down): Android's vertical scroll axis is positive upward.
     */
    fun verticalScroll(notches: Int): Float = -notches.coerceIn(-MAX_NOTCHES, MAX_NOTCHES).toFloat()

    /** The key a global action presses in a window: Back only; Home and Recents belong to the phone's screen. */
    fun globalKey(action: String): Int? = if (action == "back") KEYCODE_BACK else null

    /** What a window's input is: one event of a control batch, for one window. */
    sealed interface Event {
        data class Touch(val action: Int, val x: Int, val y: Int) : Event

        /** One finger of several at once (keys to taps); a [Touch] is the finger 0. */
        data class Pointer(val pointer: Int, val action: Int, val x: Int, val y: Int) : Event
        data class Scroll(val x: Int, val y: Int, val notches: Int) : Event
        data class Key(val code: Int) : Event
        data class Text(val text: String) : Event
        data object Close : Event
    }

    /** One event of a batch and the window it is for. */
    data class For(val window: Int, val event: Event)

    /**
     * What a control batch (the JSON `core::streaming::cast_control` makes)
     * asks of app windows: each event after a window event, with that
     * window. Null when the batch is not for a window (its first event is
     * not a window event), so the phone's screen takes it. Events a window
     * does not take (a global action but Back) are left out, and so is
     * anything before a window is named or for a window past the cap.
     */
    fun forWindows(batch: JSONObject): List<For>? {
        val events = batch.optJSONArray("events") ?: return null
        if (events.length() == 0 || events.optJSONObject(0)?.optString("type") != "window") return null
        var window = 0
        val out = ArrayList<For>()
        for (i in 0 until events.length()) {
            val e = events.optJSONObject(i) ?: continue
            if (e.optString("type") == "window") {
                window = e.optInt("window", 0).takeIf { it in 1..WindowRule.MAX_WINDOWS } ?: 0
                continue
            }
            if (window == 0) continue
            if (out.size >= MAX_EVENTS_PER_BATCH) break
            parse(e)?.let { out.add(For(window, it)) }
        }
        return out
    }

    /** The event one object of a batch is, or null when it is not one a window takes. */
    fun parse(e: JSONObject): Event? = when (e.optString("type")) {
        "touch" -> motionAction(e.optString("action"))?.let { Event.Touch(it, e.optInt("x"), e.optInt("y")) }
        "pointer" -> e.optInt("pointer", -1).takeIf { it in 0 until MAX_POINTERS }?.let { id ->
            motionAction(e.optString("action"))?.let { Event.Pointer(id, it, e.optInt("x"), e.optInt("y")) }
        }
        "scroll" -> Event.Scroll(e.optInt("x"), e.optInt("y"), e.optInt("dy"))
        "text" -> e.optString("text").takeIf { it.isNotEmpty() }?.let { Event.Text(it.take(MAX_TEXT)) }
        "backspace" -> Event.Key(KEYCODE_DEL)
        "enter" -> Event.Key(KEYCODE_ENTER)
        "global" -> globalKey(e.optString("action"))?.let { Event.Key(it) }
        "close" -> Event.Close
        else -> null
    }
}

/**
 * What the phone lets through from the computer to app windows, per second: batches in full,
 * and every event that is not a finger going down or up (moves, wheel, keys, text). Fingers going
 * down are limited per window ([WindowInput.MAX_DOWNS_PER_SECOND]) by [PointerTracker.limited].
 */
class WindowLimits {
    private val moves = dev.pocketdesk.android.control.ControlPlan.Rate()
    private val batches = dev.pocketdesk.android.control.ControlPlan.Rate(WindowInput.MAX_BATCHES_PER_SECOND)

    /** Whether a batch arriving at [nowMs] is read in full. */
    fun batch(nowMs: Long): Boolean = batches.take(nowMs)

    /** Whether one move, wheel step, key or text event at [nowMs] goes on. */
    fun move(nowMs: Long): Boolean = moves.take(nowMs)
}

/**
 * Keys to taps: the fingers of one window, as the computer sends them one
 * event at a time, turned into the whole-screen events Android wants: every
 * finger down in each event, the first as DOWN, the next as POINTER_DOWN, a
 * finger lifted as POINTER_UP (still listed in that event), the last as UP.
 * Pure, for JVM tests. Points stay fractions of the picture; the caller
 * makes pixels.
 */
class PointerTracker {
    /** What to inject: Android's [action] (with the finger's index in its high bits), [ids] and points in index order. */
    class Frame(
        val action: Int,
        val ids: IntArray,
        val xs: IntArray,
        val ys: IntArray,
        val downTime: Long,
    )

    private val ids = ArrayList<Int>()
    private val xs = ArrayList<Int>()
    private val ys = ArrayList<Int>()
    private var downTime = 0L

    /** Whether any finger is down. */
    val active: Boolean get() = ids.isNotEmpty()

    /**
     * [apply] for input from the computer, with what each result costs asked of the limits first:
     * a finger going down needs [downOk] (a limit per window); a move, and a down of a finger that
     * is already down (which is only a move), needs [moveOk]; a finger lifted is never refused, and
     * lifts only a finger that is down, so lifts are bounded by the downs let through. A refused
     * event changes nothing here.
     */
    fun limited(
        pointer: Int,
        action: Int,
        x: Int,
        y: Int,
        now: Long,
        downOk: (Long) -> Boolean,
        moveOk: (Long) -> Boolean,
    ): Frame? {
        if (pointer !in 0 until WindowInput.MAX_POINTERS) return null
        val at = ids.indexOf(pointer)
        when (action) {
            WindowInput.ACTION_DOWN -> {
                if (at < 0) {
                    if (ids.size >= WindowInput.MAX_POINTERS || !downOk(now)) return null
                } else if ((xs[at] == x && ys[at] == y) || !moveOk(now)) {
                    return null
                }
            }
            WindowInput.ACTION_MOVE -> {
                if (at < 0 || (xs[at] == x && ys[at] == y) || !moveOk(now)) return null
            }
        }
        return apply(pointer, action, x, y, now)
    }

    /** How many fingers are down. */
    val count: Int get() = ids.size

    /** The frame for finger [pointer] doing [action] at ([x], [y]) at [now]; null when it means nothing now. */
    fun apply(pointer: Int, action: Int, x: Int, y: Int, now: Long): Frame? {
        if (pointer !in 0 until WindowInput.MAX_POINTERS) return null
        val at = ids.indexOf(pointer)
        when (action) {
            WindowInput.ACTION_DOWN -> {
                if (at >= 0) return moved(at, x, y)
                if (ids.size >= WindowInput.MAX_POINTERS) return null
                val first = ids.isEmpty()
                if (first) downTime = now
                ids.add(pointer); xs.add(x); ys.add(y)
                val index = ids.size - 1
                return frame(
                    if (first) WindowInput.ACTION_DOWN
                    else WindowInput.ACTION_POINTER_DOWN or (index shl WindowInput.ACTION_POINTER_INDEX_SHIFT),
                )
            }
            WindowInput.ACTION_MOVE -> return if (at < 0) null else moved(at, x, y)
            WindowInput.ACTION_UP -> {
                if (at < 0) return null
                xs[at] = x; ys[at] = y
                val out = frame(
                    if (ids.size == 1) WindowInput.ACTION_UP
                    else WindowInput.ACTION_POINTER_UP or (at shl WindowInput.ACTION_POINTER_INDEX_SHIFT),
                )
                ids.removeAt(at); xs.removeAt(at); ys.removeAt(at)
                return out
            }
        }
        return null
    }

    /** Every finger lifted, one after another: the frames that do it (a window closing, a lost connection). */
    fun releaseAll(): List<Frame> {
        val out = ArrayList<Frame>()
        while (ids.isNotEmpty()) {
            apply(ids.last(), WindowInput.ACTION_UP, xs.last(), ys.last(), downTime)?.let { out.add(it) }
        }
        return out
    }

    private fun moved(at: Int, x: Int, y: Int): Frame? {
        if (xs[at] == x && ys[at] == y) return null
        xs[at] = x; ys[at] = y
        return frame(WindowInput.ACTION_MOVE)
    }

    private fun frame(action: Int) =
        Frame(action, ids.toIntArray(), xs.toIntArray(), ys.toIntArray(), downTime)
}
