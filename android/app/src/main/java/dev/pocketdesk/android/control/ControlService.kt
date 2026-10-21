package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.app.KeyguardManager
import android.graphics.Path
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo

/**
 * Pocketdesk's control from the computer (C8): performs the gestures,
 * global actions and typing the computer sends while the phone casts its
 * screen and the owner allows it. It reads only the focused field (for
 * text) and listens for no events. Everything runs on the main thread, as
 * the gesture callbacks do.
 */
class ControlService : AccessibilityService() {
    private val main = Handler(Looper.getMainLooper())
    /** Typing waits on the app that has the field: never on the main thread (C8 review). */
    private val typing = HandlerThread("control-typing").apply { start() }
    private val text = Handler(typing.looper)
    /** A password field's text as typed here: Android hides it from this service. */
    private var secret: Pair<AccessibilityNodeInfo, String>? = null
    private val strokes = ControlPlan.Strokes()
    /** The stroke the next segment continues, while a finger is down. */
    private var stroke: GestureDescription.StrokeDescription? = null
    /** A segment is being performed. */
    private var busy = false
    /** A finger is down, as the main thread last saw: read by the runner's thread. */
    @Volatile private var fingerDown = false

    override fun onServiceConnected() {
        instance = this
        Log.i(TAG, "control service connected")
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        if (instance === this) instance = null
        release()
        return super.onUnbind(intent)
    }

    /**
     * Lift a finger that is down (C8 review): the cast ended, control went
     * off, or the service is going. A continued stroke left with its
     * finger down would stay a touch on the screen.
     */
    fun release() = main.post {
        strokes.lift(SystemClock.uptimeMillis())
        pump()
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        typing.quitSafely()
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    /** The screen's size as it is drawn now, rotation included. */
    private fun screen(): Pair<Int, Int> {
        val display = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        display.getRealMetrics(metrics)
        return metrics.widthPixels to metrics.heightPixels
    }

    /**
     * Whether the phone is locked: then nothing is touched or typed, so the
     * computer cannot enter its PIN or pattern (C8 review); Back and Home
     * still go. A finger already down is lifted.
     */
    private fun locked(): Boolean = getSystemService(KeyguardManager::class.java).isDeviceLocked

    /** A touch at a fraction of the screen ([ControlPlan.place]). */
    fun touch(action: String, x: Int, y: Int): Boolean = main.post {
        if (locked()) {
            strokes.lift(SystemClock.uptimeMillis())
            pump()
            return@post
        }
        val (w, h) = screen()
        val point = ControlPlan.place(x, y, w, h)
        val now = SystemClock.uptimeMillis()
        when (action) {
            "down" -> {
                // A press while one is down (its release was lost): the old
                // finger lifts first, or Android keeps its stroke going.
                if (strokes.active || stroke != null) {
                    strokes.lift(now)
                    pump()
                }
                if (busy || stroke != null) {
                    // The lift is still performed: this press waits for it.
                    main.postDelayed({ touch(action, x, y) }, ControlPlan.MIN_SEGMENT_MS)
                    return@post
                }
                strokes.down(point, now)
            }
            "move" -> strokes.move(point, now)
            "up" -> strokes.up(point, now)
        }
        pump()
    }

    /** Lift a forgotten finger, and start a press held still ([ControlPlan.START_WAIT_MS]). */
    fun tick() = main.post {
        strokes.liftIfIdle(SystemClock.uptimeMillis())
        pump()
    }

    /** Whether a finger is down, so the runner looks again soon (a press to start, a finger to lift). */
    fun waiting(): Boolean = fingerDown

    private fun pump() {
        fingerDown = strokes.active
        if (busy) return
        val segment = strokes.next(SystemClock.uptimeMillis()) ?: return
        perform(segment)
    }

    private fun perform(segment: ControlPlan.Segment) {
        val path = Path().apply {
            moveTo(segment.points.first().x, segment.points.first().y)
            segment.points.drop(1).forEach { lineTo(it.x, it.y) }
        }
        val previous = stroke
        val next = try {
            if (segment.start || previous == null) {
                GestureDescription.StrokeDescription(path, 0, segment.durationMs, segment.willContinue)
            } else {
                previous.continueStroke(path, 0, segment.durationMs, segment.willContinue)
            }
        } catch (e: IllegalArgumentException) {
            // A path Android will not take (off its screen): the gesture ends.
            Log.w(TAG, "a segment was refused: ${e.message}")
            stroke = null
            strokes.cancelled()
            return
        }
        stroke = if (segment.willContinue) next else null
        busy = true
        val sent = dispatchGesture(
            GestureDescription.Builder().addStroke(next).build(),
            object : GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) {
                    busy = false
                    pump()
                }

                override fun onCancelled(gestureDescription: GestureDescription?) {
                    busy = false
                    stroke = null
                    strokes.cancelled()
                }
            },
            main,
        )
        if (!sent) {
            busy = false
            stroke = null
            strokes.cancelled()
        }
    }

    /** A wheel's notches at a fraction of the screen, as a swipe; not while a finger is down. */
    fun scroll(x: Int, y: Int, notches: Int) = main.post {
        if (strokes.active || busy || notches == 0 || locked()) return@post
        val (w, h) = screen()
        val points = ControlPlan.scroll(ControlPlan.place(x, y, w, h), notches, h)
        perform(ControlPlan.Segment(points, SCROLL_MS, start = true, willContinue = false))
    }

    /** Back, Home, Recents or the notification shade. */
    fun global(action: String) = main.post { pressGlobal(action) }

    /**
     * M4: [global] in turn with what the computer types, on the thread that
     * types (M4 review: on the main thread a Home could overtake the text
     * typed before it).
     */
    fun globalBlind(action: String) = text.post { pressGlobal(action) }

    private fun pressGlobal(action: String) {
        val code = when (action) {
            "back" -> GLOBAL_ACTION_BACK
            "home" -> GLOBAL_ACTION_HOME
            "recents" -> GLOBAL_ACTION_RECENTS
            "notifications" -> GLOBAL_ACTION_NOTIFICATIONS
            else -> return
        }
        // Locked, only Back and Home: the shade and Recents could reach
        // what the lock keeps.
        if (locked() && code != GLOBAL_ACTION_BACK && code != GLOBAL_ACTION_HOME) return
        performGlobalAction(code)
    }

    /**
     * The field that takes typing now, if any (none while locked), read fresh: this service
     * listens for no text events, so Android's cached copy can still hold
     * the text before the last edit (a Backspace right after typing undid
     * the typing).
     */
    private fun field(): AccessibilityNodeInfo? =
        if (locked()) null else rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            ?.also { it.refresh() }
            ?.takeIf { it.isEditable }

    /**
     * What the field holds: its hint is not its text, and a password field's
     * is what was typed into it here (from empty when it was not).
     */
    private fun textOf(node: AccessibilityNodeInfo): String = when {
        node.isPassword -> secret?.takeIf { it.first == node }?.second.orEmpty()
        node.isShowingHintText -> ""
        else -> node.text?.toString().orEmpty()
    }

    /** Where a field's selection is; a password field's is at its end. */
    private fun selectionOf(node: AccessibilityNodeInfo): Pair<Int, Int> =
        if (node.isPassword) -1 to -1 else node.textSelectionStart to node.textSelectionEnd

    private fun setText(node: AccessibilityNodeInfo, edited: ControlPlan.Edited) {
        secret = if (node.isPassword) node to edited.text else null
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_TEXT,
            Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, edited.text)
            },
        )
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, edited.cursor)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, edited.cursor)
            },
        )
    }

    /** [typed] at the focused field's selection. */
    fun type(typed: String) = text.post {
        val node = field() ?: return@post
        val (start, end) = selectionOf(node)
        setText(node, ControlPlan.type(textOf(node), start, end, typed))
    }

    fun backspace() = text.post {
        val node = field() ?: return@post
        val (start, end) = selectionOf(node)
        setText(node, ControlPlan.backspace(textOf(node), start, end))
    }

    /** The field's action (send, search, next), on Android 11+; else a new line where one is allowed. */
    fun enter() = text.post {
        val node = field() ?: return@post
        if (Build.VERSION.SDK_INT >= 30 &&
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
        ) {
            return@post
        }
        if (node.isMultiLine) {
            val (start, end) = selectionOf(node)
            setText(node, ControlPlan.type(textOf(node), start, end, "\n"))
        }
    }

    /**
     * The field to type in from the computer's keyboard alone (M4), where
     * the computer sees nothing of the screen: never a password field, which
     * [refused] is told of instead.
     */
    private fun blindField(refused: () -> Unit): AccessibilityNodeInfo? {
        val node = field() ?: return null
        if (node.isPassword) {
            refused()
            return null
        }
        return node
    }

    /** M4: [typed] as [type], never into a password field. */
    fun typeBlind(typed: String, refused: () -> Unit) = text.post {
        val node = blindField(refused) ?: return@post
        val (start, end) = selectionOf(node)
        setText(node, ControlPlan.type(textOf(node), start, end, typed))
    }

    /** M4: [backspace], never in a password field. */
    fun backspaceBlind(refused: () -> Unit) = text.post {
        val node = blindField(refused) ?: return@post
        val (start, end) = selectionOf(node)
        setText(node, ControlPlan.backspace(textOf(node), start, end))
    }

    /** M4: [enter], never in a password field. */
    fun enterBlind(refused: () -> Unit) = text.post {
        val node = blindField(refused) ?: return@post
        if (Build.VERSION.SDK_INT >= 30 &&
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
        ) {
            return@post
        }
        if (node.isMultiLine) {
            val (start, end) = selectionOf(node)
            setText(node, ControlPlan.type(textOf(node), start, end, "\n"))
        }
    }

    companion object {
        private const val TAG = "ControlService"
        private const val SCROLL_MS = 120L

        /** The running service, while Android has it bound. */
        @Volatile
        var instance: ControlService? = null
            private set
    }
}
