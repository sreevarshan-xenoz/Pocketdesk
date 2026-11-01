package dev.pocketdesk.android.windows

import android.annotation.SuppressLint
import android.content.Context
import android.content.ContextWrapper
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import android.view.InputDevice
import android.view.InputEvent
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import java.lang.reflect.Method
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import kotlin.system.exitProcess

/**
 * Phone apps tier 2 (separate windows): the helper process Shizuku starts
 * with shell identity (uid 2000). It does three things and no more: makes a
 * display for an app window, starts an app on that display, and sends input
 * to that display. It runs only while windows are open, and exits when it is
 * told to or its last display is released.
 *
 * The technique is that of scrcpy (Apache-2.0, Copyright Genymobile): a
 * display made from a context that names `com.android.shell`, flags that make
 * it an own, trusted, touch-taking display, and input events given a display
 * id before they are injected through the input service. Written for this
 * app, not copied; see NOTICE. The same approach, with Shizuku starting the
 * process, is Ynkcc/VirtualDisplay (Apache-2.0).
 *
 * Everything here is reflection on hidden Android APIs, which a shell
 * process may use; each step answers false or -1 rather than throwing, and
 * the app then opens the app the ordinary way.
 */
@SuppressLint("PrivateApi", "DiscouragedPrivateApi", "SoonBlockedPrivateApi")
class WindowHostService() : IWindowHost.Stub() {
    /** Shizuku (13.1 and later) hands a user service its [Context]; older ones use the constructor above. */
    constructor(context: Context) : this() {
        // This app's uid, when the context says it (an app's, never the shell's).
        expectedUid = runCatching { context.applicationInfo.uid }.getOrDefault(-1).takeIf { it > WindowRule.UID_SHELL } ?: -1
    }

    /**
     * The one uid whose calls are taken: this app's. Learnt from the first call when the
     * context did not say; Shizuku hands the binder to the app that bound the service only.
     */
    @Volatile
    private var expectedUid = -1

    /** Refuse a call that is not from this app (or, for [destroy], from Shizuku's own server). */
    private fun guard(destroy: Boolean = false) {
        val caller = Binder.getCallingUid()
        if (expectedUid < 0 && !destroy && caller > WindowRule.UID_SHELL) expectedUid = caller
        if (!WindowRule.callerAllowed(caller, expectedUid, destroy)) throw SecurityException("a call from $caller")
    }

    private val displays = ConcurrentHashMap<Int, VirtualDisplay>()

    /** A context whose package is the shell's, so the display service accepts the calls. */
    private val shellContext: Context? by lazy {
        runCatching {
            val activityThread = Class.forName("android.app.ActivityThread")
            val thread = runCatching { activityThread.getMethod("currentActivityThread").invoke(null) }.getOrNull()
                ?: activityThread.getMethod("systemMain").invoke(null)
            ShellContext(activityThread.getMethod("getSystemContext").invoke(thread) as Context)
        }.onFailure { Log.w(TAG, "no shell context", it) }.getOrNull()
    }

    private val displayManager: DisplayManager? by lazy {
        runCatching {
            val constructor = DisplayManager::class.java.getDeclaredConstructor(Context::class.java)
            constructor.isAccessible = true
            constructor.newInstance(shellContext)
        }.onFailure { Log.w(TAG, "no display manager", it) }.getOrNull()
    }

    override fun createDisplay(name: String, width: Int, height: Int, dpi: Int, surface: Surface?): Int {
        guard()
        if (surface == null || !surface.isValid) return -1
        return runCatching {
            val manager = displayManager ?: return -1
            val display = manager.createVirtualDisplay(name, width, height, dpi, surface, WindowRule.displayFlags(Build.VERSION.SDK_INT))
                ?: return -1
            val id = display.display.displayId
            displays[id] = display
            id
        }.onFailure { Log.w(TAG, "no display", it) }.getOrDefault(-1)
    }

    override fun launch(displayId: Int, component: String): Boolean {
        guard()
        if (!displays.containsKey(displayId) || !WindowRule.componentOk(component)) return false
        return runCatching {
            val process = ProcessBuilder("/system/bin/am", "start", "--display", displayId.toString(), "-n", component)
                .redirectErrorStream(true)
                .start()
            // Waited for first, with a limit, then read: `am` writes a few lines and exits.
            val finished = process.waitFor(LAUNCH_WAIT_S, TimeUnit.SECONDS)
            if (!finished) {
                process.destroy()
                return false
            }
            val output = process.inputStream.bufferedReader().readText()
            process.exitValue() == 0 && !output.contains("Error")
        }.onFailure { Log.w(TAG, "no launch", it) }.getOrDefault(false)
    }

    override fun release(displayId: Int) {
        guard()
        displays.remove(displayId)?.let { runCatching { it.release() } }
    }

    override fun injectTouch(displayId: Int, action: Int, x: Float, y: Float, downTime: Long): Boolean {
        guard()
        if (!(action == WindowInput.ACTION_DOWN || action == WindowInput.ACTION_UP || action == WindowInput.ACTION_MOVE)) return false
        if (!displays.containsKey(displayId)) return false
        val event = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), action, x, y, 0)
        event.source = InputDevice.SOURCE_TOUCHSCREEN
        return inject(event, displayId)
    }

    override fun injectScroll(displayId: Int, x: Float, y: Float, vscroll: Float): Boolean {
        guard()
        if (!displays.containsKey(displayId)) return false
        val now = SystemClock.uptimeMillis()
        val properties = arrayOf(MotionEvent.PointerProperties().apply { id = 0; toolType = MotionEvent.TOOL_TYPE_MOUSE })
        val coords = arrayOf(
            MotionEvent.PointerCoords().apply {
                this.x = x
                this.y = y
                setAxisValue(MotionEvent.AXIS_VSCROLL, vscroll)
            },
        )
        val event = MotionEvent.obtain(
            now, now, WindowInput.ACTION_SCROLL, 1, properties, coords, 0, 0, 1f, 1f, 0, 0,
            InputDevice.SOURCE_MOUSE, 0,
        )
        return inject(event, displayId)
    }

    override fun injectKey(displayId: Int, keyCode: Int): Boolean {
        guard()
        // Back, Enter and Delete only: nothing that would leave the app (Home, Power, Recents).
        if (!displays.containsKey(displayId) || !WindowRule.keyAllowed(keyCode)) return false
        val now = SystemClock.uptimeMillis()
        return listOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP).all { action ->
            inject(
                KeyEvent(now, now, action, keyCode, 0, 0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, InputDevice.SOURCE_KEYBOARD),
                displayId,
            )
        }
    }

    override fun injectText(displayId: Int, text: String): Boolean {
        guard()
        if (!displays.containsKey(displayId) || text.isEmpty()) return false
        val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(text.toCharArray()) ?: return false
        return events.all { inject(it, displayId) }
    }

    override fun destroy() {
        guard(destroy = true)
        displays.keys.toList().forEach(::release)
        exitProcess(0)
    }

    /** The input service, as IInputManager (it is there on every Android this runs on). */
    private val input: Any? by lazy {
        runCatching {
            val binder = Class.forName("android.os.ServiceManager")
                .getMethod("getService", String::class.java)
                .invoke(null, "input") as IBinder
            Class.forName("android.hardware.input.IInputManager\$Stub")
                .getMethod("asInterface", IBinder::class.java)
                .invoke(null, binder)
        }.onFailure { Log.w(TAG, "no input service", it) }.getOrNull()
    }

    private val injectMethod: Method? by lazy {
        runCatching { input?.javaClass?.getMethod("injectInputEvent", InputEvent::class.java, Int::class.javaPrimitiveType) }
            .getOrNull()
    }

    private val setDisplayId: Method? by lazy {
        runCatching { InputEvent::class.java.getMethod("setDisplayId", Int::class.javaPrimitiveType) }.getOrNull()
    }

    /** Give [event] its display and inject it, without waiting for the app to take it. */
    private fun inject(event: InputEvent, displayId: Int): Boolean = runCatching {
        // `setDisplayId` returns nothing: only its presence decides (see WindowRule.inject).
        val setDisplay = setDisplayId?.let { method -> { e: InputEvent, id: Int -> method.invoke(e, id); Unit } }
        val send = injectMethod?.let { method -> { e: InputEvent -> method.invoke(input, e, INJECT_ASYNC) as? Boolean ?: false } }
        WindowRule.inject(event, displayId, setDisplay, send)
    }.onFailure { Log.w(TAG, "no injection", it) }.getOrDefault(false)

    /** The shell's own package name, so the platform's package checks pass for uid 2000. */
    private class ShellContext(base: Context) : ContextWrapper(base) {
        override fun getPackageName(): String = SHELL
        override fun getOpPackageName(): String = SHELL
        override fun getApplicationContext(): Context = this
    }

    private companion object {
        const val TAG = "WindowHost"
        const val SHELL = "com.android.shell"
        const val LAUNCH_WAIT_S = 10L
        const val INJECT_ASYNC = 0
    }
}
