package dev.pocketdesk.android.control

/**
 * The phone as another screen for its computer's keyboard and mouse (BK5):
 * what each event the computer sends becomes here, kept apart from Android
 * so it is tested. Positions are fractions out of [ControlPlan.FRACTION_MAX]
 * of the screen, as C8's touches are.
 */
object PointerPlan {
    /** Nothing from the computer this long (it pings five times a second): it has gone. */
    const val SILENCE_MS = 1_000L

    /** The pointer hides after this long without a move, the visit going on. */
    const val IDLE_HIDE_MS = 3_000L

    /** One event from the computer, as read from its batch. */
    data class Event(
        val type: String,
        val x: Int = 0,
        val y: Int = 0,
        val button: String = "",
        val action: String = "",
        val dx: Int = 0,
        val dy: Int = 0,
        val text: String = "",
        val key: String = "",
    )

    /** What to do here. */
    sealed interface Act {
        /** Show the pointer at this fraction of the screen. */
        data class Pointer(val x: Int, val y: Int) : Act

        data object Hide : Act

        /** A finger: down, move or up, as C8's touches. */
        data class Touch(val action: String, val x: Int, val y: Int) : Act

        /** A wheel's notches as a swipe there, positive down. */
        data class Scroll(val x: Int, val y: Int, val notches: Int) : Act

        /** Back or Home. */
        data class Global(val action: String) : Act

        /** Text into the field in front. */
        data class Text(val text: String) : Act

        /** A key in the field in front: enter, backspace, delete, left, right, up, down. */
        data class Key(val key: String) : Act
    }

    enum class Decision { TAKE, OFF, SERVICE_OFF, NOT_OWN_COMPUTER }

    /**
     * Take what the computer sends? With "Let my computer's keyboard and
     * mouse use this phone" [on], C8's Accessibility [service] running, and
     * from the phone's [own] computer.
     */
    fun decide(on: Boolean, service: Boolean, own: Boolean): Decision = when {
        !on -> Decision.OFF
        !own -> Decision.NOT_OWN_COMPUTER
        !service -> Decision.SERVICE_OFF
        else -> Decision.TAKE
    }

    /** One visit of the computer's cursor: where it is, and a finger down. */
    class Pointer {
        /** The computer's cursor is on this phone. */
        var on = false
            private set
        private var x = 0
        private var y = 0
        private var fingerDown = false
        private var shown = false
        private var heardMs = 0L
        private var movedMs = 0L

        /** What [event], come at [nowMs], does here. */
        fun take(event: Event, nowMs: Long): List<Act> {
            if (event.type == "leave") return release()
            // Anything else says the cursor is here, an enter missed or not.
            on = true
            heardMs = nowMs
            return when (event.type) {
                "enter", "pointer" -> {
                    x = event.x
                    y = event.y
                    shown = true
                    movedMs = nowMs
                    if (fingerDown) listOf(Act.Pointer(x, y), Act.Touch("move", x, y)) else listOf(Act.Pointer(x, y))
                }
                "button" -> button(event.button, event.action == "down")
                "wheel" -> if (event.dy != 0) listOf(Act.Scroll(x, y, event.dy)) else emptyList()
                "text" -> if (event.text.isNotEmpty()) listOf(Act.Text(event.text)) else emptyList()
                "key" -> when (event.key) {
                    "escape" -> listOf(Act.Global("back"))
                    in KEYS -> listOf(Act.Key(event.key))
                    else -> emptyList()
                }
                else -> emptyList()
            }
        }

        private fun button(button: String, down: Boolean): List<Act> = when (button) {
            "left" -> when {
                down -> {
                    fingerDown = true
                    listOf(Act.Touch("down", x, y))
                }
                fingerDown -> {
                    fingerDown = false
                    listOf(Act.Touch("up", x, y))
                }
                else -> emptyList()
            }
            // On the press, as a phone's own buttons act.
            "right" -> if (down) listOf(Act.Global("back")) else emptyList()
            "middle" -> if (down) listOf(Act.Global("home")) else emptyList()
            else -> emptyList()
        }

        /**
         * The time is [nowMs]: nothing from the computer for [SILENCE_MS]
         * lets go of everything; a pointer still for [IDLE_HIDE_MS], with no
         * finger down, hides until it moves.
         */
        fun tick(nowMs: Long): List<Act> {
            if (!on) return emptyList()
            if (nowMs - heardMs >= SILENCE_MS) return release()
            if (shown && !fingerDown && nowMs - movedMs >= IDLE_HIDE_MS) {
                shown = false
                return listOf(Act.Hide)
            }
            return emptyList()
        }

        /** The cursor left, or the phone stops taking it: a finger down lifts, the pointer hides. */
        fun release(): List<Act> {
            val acts = mutableListOf<Act>()
            if (fingerDown) acts += Act.Touch("up", x, y)
            if (shown) acts += Act.Hide
            fingerDown = false
            shown = false
            on = false
            return acts
        }
    }

    /** The keys that act in the field in front. */
    private val KEYS = setOf("enter", "backspace", "delete", "left", "right", "up", "down")

    /** Delete in [text] at its selection: the selection, else the character after the cursor. */
    fun deleteForward(text: String, start: Int, end: Int): ControlPlan.Edited {
        val (from, to) = selection(text, start, end)
        if (from != to) return ControlPlan.Edited(text.substring(0, from) + text.substring(to), from)
        if (from >= text.length) return ControlPlan.Edited(text, from)
        // A whole character: a surrogate pair is one.
        val after = Character.offsetByCodePoints(text, from, 1)
        return ControlPlan.Edited(text.substring(0, from) + text.substring(after), from)
    }

    /** Where the cursor goes for an arrow [delta] (-1 left, 1 right) in [text] at its selection. */
    fun caret(text: String, start: Int, end: Int, delta: Int): Int {
        val (from, to) = selection(text, start, end)
        if (from != to) return if (delta < 0) from else to
        return when {
            delta < 0 && from > 0 -> Character.offsetByCodePoints(text, from, -1)
            delta > 0 && from < text.length -> Character.offsetByCodePoints(text, from, 1)
            else -> from
        }
    }

    /** A selection in [text], in order; none known (-1) is the end. */
    private fun selection(text: String, start: Int, end: Int): Pair<Int, Int> {
        if (start < 0 || end < 0) return text.length to text.length
        val a = start.coerceIn(0, text.length)
        val b = end.coerceIn(0, text.length)
        return minOf(a, b) to maxOf(a, b)
    }
}
