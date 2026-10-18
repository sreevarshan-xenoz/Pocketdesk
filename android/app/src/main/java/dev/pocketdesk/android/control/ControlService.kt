package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
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
    private val strokes = ControlPlan.Strokes()
    /** The stroke the next segment continues, while a finger is down. */
    private var stroke: GestureDescription.StrokeDescription? = null
    /** A segment is being performed. */
    private var busy = false

    override fun onServiceConnected() {
        instance = this
        Log.i(TAG, "control service connected")
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        if (instance === this) instance = null
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        if (instance === this) instance = null
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

    /** A touch at a fraction of the screen ([ControlPlan.place]). */
    fun touch(action: String, x: Int, y: Int) = main.post {
        val (w, h) = screen()
        val point = ControlPlan.place(x, y, w, h)
        val now = SystemClock.uptimeMillis()
        when (action) {
            "down" -> strokes.down(point, now)
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

    /** Whether a press waits to start or more is pending. */
    fun waiting(): Boolean = strokes.waiting(SystemClock.uptimeMillis())

    private fun pump() {
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
        if (strokes.active || busy || notches == 0) return@post
        val (w, h) = screen()
        val points = ControlPlan.scroll(ControlPlan.place(x, y, w, h), notches, h)
        perform(ControlPlan.Segment(points, SCROLL_MS, start = true, willContinue = false))
    }

    /** Back, Home, Recents or the notification shade. */
    fun global(action: String) = main.post {
        val code = when (action) {
            "back" -> GLOBAL_ACTION_BACK
            "home" -> GLOBAL_ACTION_HOME
            "recents" -> GLOBAL_ACTION_RECENTS
            "notifications" -> GLOBAL_ACTION_NOTIFICATIONS
            else -> return@post
        }
        performGlobalAction(code)
    }

    /** The field that takes typing now, if any. */
    private fun field(): AccessibilityNodeInfo? =
        rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.takeIf { it.isEditable }

    /** What the field holds: its hint is not its text. */
    private fun textOf(node: AccessibilityNodeInfo): String =
        if (node.isShowingHintText) "" else node.text?.toString().orEmpty()

    private fun setText(node: AccessibilityNodeInfo, edited: ControlPlan.Edited) {
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

    /** [text] typed at the focused field's selection. */
    fun type(text: String) = main.post {
        val node = field() ?: return@post
        setText(node, ControlPlan.type(textOf(node), node.textSelectionStart, node.textSelectionEnd, text))
    }

    fun backspace() = main.post {
        val node = field() ?: return@post
        setText(node, ControlPlan.backspace(textOf(node), node.textSelectionStart, node.textSelectionEnd))
    }

    /** The field's action (send, search, next), on Android 11+; else a new line where one is allowed. */
    fun enter() = main.post {
        val node = field() ?: return@post
        if (Build.VERSION.SDK_INT >= 30 &&
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
        ) {
            return@post
        }
        if (node.isMultiLine) {
            setText(node, ControlPlan.type(textOf(node), node.textSelectionStart, node.textSelectionEnd, "\n"))
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
