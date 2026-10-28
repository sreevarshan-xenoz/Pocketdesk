package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.accessibilityservice.GestureDescription
import android.app.KeyguardManager
import android.graphics.Path
import android.graphics.PixelFormat
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
import android.view.Gravity
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo

/**
 * Pocketdesk's control from the computer (C8): performs the gestures,
 * global actions and typing the computer sends while the phone casts its
 * screen and the owner allows it. It reads only the focused field (for
 * text), and listens for no events but SystemUI's windows while "Send my
 * copies by themselves" is on (M10). Everything runs on the main thread, as
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
        syncCopyWatch(dev.pocketdesk.android.Prefs.copiesByThemselves(this))
    }

    /** M10: SystemUI's windows are watched, for its clipboard overlay. */
    private var watchingCopies = false
    private var lastCopyReadMs = 0L

    /**
     * Listen for SystemUI's window changes only while "Send my copies by
     * themselves" is [on] (M10); otherwise for nothing, as C8 does.
     */
    fun syncCopyWatch(wanted: Boolean) = main.post {
        // Only where SystemUI shows the overlay (M10 review: below 13 it
        // watched every SystemUI window for nothing).
        val on = wanted && dev.pocketdesk.android.clip.CopyWatchRule.supported(Build.VERSION.SDK_INT)
        if (on == watchingCopies) return@post
        val info = serviceInfo ?: return@post
        if (on) {
            // A copy made while the preview is still up changes its
            // contents, not its window (M10 review).
            info.eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED or AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
            info.packageNames = arrayOf(dev.pocketdesk.android.clip.CopyWatchRule.SYSTEM_UI)
            info.flags = info.flags or AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or
                AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
        } else {
            info.eventTypes = 0
            info.packageNames = null
            info.flags = info.flags and
                (AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS).inv()
        }
        serviceInfo = info
        watchingCopies = on
        Log.i(TAG, "watching copies: $on")
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
        removePointer()
        typing.quitSafely()
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (!watchingCopies || event == null) return
        val stateChanged = event.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
        if (!stateChanged && event.eventType != AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED) return
        if (event.packageName != dev.pocketdesk.android.clip.CopyWatchRule.SYSTEM_UI) return
        // A read already waits for the debounce: it reads this copy too.
        if (copyReadPending) return
        // The cheap checks first (M10 review): the switch and the lock.
        if (!dev.pocketdesk.android.Prefs.copiesByThemselves(this)) return
        if (getSystemService(KeyguardManager::class.java).isDeviceLocked) return
        if (!isClipboardOverlay(event)) return
        // A change inside the preview is a new copy only if what it shows
        // changed; its own redraws are not (kept as a hash, never the text).
        val shown = runCatching { previewHash(event.source?.window?.root ?: event.source) }.getOrDefault(0)
        if (!stateChanged && shown == lastPreviewHash) return
        lastPreviewHash = shown
        if (dev.pocketdesk.android.notify.PhoneLink.target(this) == null) return
        val wait = dev.pocketdesk.android.clip.CopyWatchRule.delay(SystemClock.elapsedRealtime(), lastCopyReadMs)
        copyReadPending = true
        main.postDelayed(::readCopy, wait)
    }

    /** A read is waiting for the debounce (M10). */
    private var copyReadPending = false

    /** What the clipboard preview showed when last looked at, as a hash (M10). */
    private var lastPreviewHash = 0

    /** A hash of the texts in the preview's window, at most 60 views (M10). */
    private fun previewHash(root: AccessibilityNodeInfo?): Int {
        var hash = 17
        val queue = ArrayDeque<AccessibilityNodeInfo>()
        root?.let { queue.add(it) }
        var seen = 0
        while (queue.isNotEmpty() && seen < 60) {
            val node = queue.removeFirst()
            seen++
            hash = 31 * hash + (node.text?.toString()?.hashCode() ?: 0)
            for (i in 0 until node.childCount) node.getChild(i)?.let { queue.add(it) }
        }
        return hash
    }

    /** Start the window that reads the copy, if the switch is still on. */
    private fun readCopy() {
        copyReadPending = false
        if (!watchingCopies || getSystemService(KeyguardManager::class.java).isDeviceLocked) return
        lastCopyReadMs = SystemClock.elapsedRealtime()
        runCatching {
            startActivity(
                android.content.Intent(this, dev.pocketdesk.android.clip.CopyReadActivity::class.java)
                    .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_NO_ANIMATION),
            )
        }.onFailure { Log.w(TAG, "A copy could not be read", it) }
    }

    /**
     * Whether [event]'s own window is the clipboard overlay (M10): its title,
     * or SystemUI's clipboard views under the event's source, looked at
     * breadth first from its window's root and no further than the first
     * match or 40 views. Only view ids are read here.
     */
    private fun isClipboardOverlay(event: AccessibilityEvent): Boolean {
        val rule = dev.pocketdesk.android.clip.CopyWatchRule
        val title = runCatching { windows.firstOrNull { it.id == event.windowId }?.title?.toString() }.getOrNull()
        if (rule.isOverlayName(title)) return true
        val queue = ArrayDeque<AccessibilityNodeInfo>()
        // From its window's root: a content change's source is a view inside.
        (runCatching { event.source?.window?.root }.getOrNull() ?: event.source)?.let { queue.add(it) }
        var seen = 0
        while (queue.isNotEmpty() && seen < 40) {
            val node = queue.removeFirst()
            seen++
            if (rule.isOverlayName(node.viewIdResourceName)) return true
            for (i in 0 until node.childCount) node.getChild(i)?.let { queue.add(it) }
        }
        return false
    }

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

    /**
     * BK5: the computer holding the button down is still there: a finger
     * down stays (C8's idle lift is for a cast that stopped sending). A
     * press that landed after its own release (the down re-posted while a
     * segment ran) is never kept: the runner keeps only while its own
     * button is down, so the idle lift still takes that one.
     */
    fun keepPressed() = main.post { strokes.keep(SystemClock.uptimeMillis()) }

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

    /** BK5: the pointer the computer's mouse moves here, while it is on this phone. */
    private var pointer: PointerArrow? = null
    private var pointerParams: WindowManager.LayoutParams? = null

    /**
     * BK5: show the computer's pointer at a fraction of the screen, over
     * every app, taking no touch; never over a locked phone.
     */
    fun pointerAt(x: Int, y: Int) = main.post {
        if (locked()) {
            removePointer()
            return@post
        }
        val (w, h) = screen()
        val at = ControlPlan.place(x, y, w, h)
        val windows = getSystemService(WindowManager::class.java)
        val params = pointerParams ?: run {
            val side = (PointerArrow.SIZE_DP * resources.displayMetrics.density).toInt()
            WindowManager.LayoutParams(
                side,
                side,
                WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                    WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                    WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                    WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                PixelFormat.TRANSLUCENT,
            ).apply {
                gravity = Gravity.TOP or Gravity.START
                // Over a cutout too, so the pointer sits where a touch lands.
                if (Build.VERSION.SDK_INT >= 30) {
                    layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                } else if (Build.VERSION.SDK_INT >= 28) {
                    layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
                title = "Pocketdesk pointer"
            }.also { pointerParams = it }
        }
        params.x = at.x.toInt()
        params.y = at.y.toInt()
        val shown = pointer
        if (shown == null) {
            val view = PointerArrow(this)
            runCatching { windows.addView(view, params) }
                .onSuccess { pointer = view }
                .onFailure { Log.w(TAG, "The pointer could not be shown", it) }
        } else {
            runCatching { windows.updateViewLayout(shown, params) }
        }
    }

    /** BK5: the computer's pointer goes. */
    fun hidePointer() = main.post { removePointer() }

    private fun removePointer() {
        val shown = pointer ?: return
        pointer = null
        runCatching { getSystemService(WindowManager::class.java).removeView(shown) }
    }

    /**
     * BK5: a key from the computer's keyboard in the field in front, never
     * in a password field ([refused] is told instead): Enter and Backspace
     * as M4 does them, Delete, and the arrows moving the cursor (up and down
     * by a line, where the field moves by lines).
     */
    fun keyBlind(key: String, refused: () -> Unit) {
        when (key) {
            "enter" -> enterBlind(refused)
            "backspace" -> backspaceBlind(refused)
            "delete" -> text.post {
                val node = blindField(refused) ?: return@post
                val (start, end) = selectionOf(node)
                setText(node, PointerPlan.deleteForward(textOf(node), start, end))
            }
            "left", "right" -> text.post {
                val node = blindField(refused) ?: return@post
                val (start, end) = selectionOf(node)
                val at = PointerPlan.caret(textOf(node), start, end, if (key == "left") -1 else 1)
                node.performAction(
                    AccessibilityNodeInfo.ACTION_SET_SELECTION,
                    Bundle().apply {
                        putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, at)
                        putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, at)
                    },
                )
            }
            "up", "down" -> text.post {
                val node = blindField(refused) ?: return@post
                node.performAction(
                    if (key == "up") {
                        AccessibilityNodeInfo.ACTION_PREVIOUS_AT_MOVEMENT_GRANULARITY
                    } else {
                        AccessibilityNodeInfo.ACTION_NEXT_AT_MOVEMENT_GRANULARITY
                    },
                    Bundle().apply {
                        putInt(
                            AccessibilityNodeInfo.ACTION_ARGUMENT_MOVEMENT_GRANULARITY_INT,
                            AccessibilityNodeInfo.MOVEMENT_GRANULARITY_LINE,
                        )
                        putBoolean(AccessibilityNodeInfo.ACTION_ARGUMENT_EXTEND_SELECTION_BOOLEAN, false)
                    },
                )
            }
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
