package dev.pocketdesk.android.windows

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.cast.CastEncoder
import dev.pocketdesk.android.control.AppOpenRule

/**
 * Phone apps tier 2 (separate windows): the app windows of one cast. Each is
 * a display of its own that Shizuku's helper process makes at the size the
 * computer asked for, an app started on it, a video encoder fed by it (a
 * stream of the cast of its own), and the computer's touch and typing sent
 * back to that display. The phone's own screen is not involved.
 *
 * Opening, input and closing happen off the main thread. [onEmpty] runs when
 * the last window has closed.
 */
class WindowCast(private val context: Context, private val onEmpty: () -> Unit) {
    private class Window(
        val id: Int,
        val pkg: String,
        val encoder: CastEncoder,
        val displayId: Int,
        val width: Int,
        val height: Int,
    ) {
        /** The fingers down in this window now (one gesture, however many fingers). */
        val fingers = PointerTracker()
    }

    private val windows = java.util.concurrent.ConcurrentHashMap<Int, Window>()

    /** Windows being made, so one id is not made twice at once. */
    private val making = java.util.concurrent.ConcurrentHashMap.newKeySet<Int>()

    @Volatile
    private var closing = false

    /** The ids of the windows open or being made. */
    val ids: Set<Int> get() = windows.keys + making

    val isEmpty: Boolean get() = windows.isEmpty() && making.isEmpty()

    /**
     * Make window [ask] for [pkg], off this thread; [done] gets
     * [AppOpenRule]'s word for how it went: opened, not found, windows
     * unavailable, failed, or the window limit.
     */
    fun open(ask: WindowRule.Ask, pkg: String, done: (String) -> Unit) {
        if (closing) return done(AppOpenRule.WINDOWS_UNAVAILABLE)
        // Being made already is as good as open: one id is never made twice.
        if (!making.add(ask.id)) return done(AppOpenRule.WINDOW_LIMIT)
        if (windows.containsKey(ask.id) || windows.size + making.size - 1 >= WindowRule.MAX_WINDOWS) {
            making.remove(ask.id)
            return done(AppOpenRule.WINDOW_LIMIT)
        }
        // Never the phone's own settings, installer, permission screens, Shizuku or Pocketdesk.
        if (WindowRule.denied(pkg, context.packageName, deniedPackages(context))) {
            making.remove(ask.id)
            return done(AppOpenRule.WINDOWS_UNAVAILABLE)
        }
        val component = context.packageManager.getLaunchIntentForPackage(pkg)?.component?.flattenToShortString()
        if (component == null || !WindowRule.componentOk(component)) {
            making.remove(ask.id)
            return done(AppOpenRule.NOT_FOUND)
        }
        ShizukuBridge.host(context) { host ->
            Thread({ make(host, ask, pkg, component, done) }, "window-open").start()
        }
    }

    private fun make(host: IWindowHost?, ask: WindowRule.Ask, pkg: String, component: String, done: (String) -> Unit) {
        val result = try {
            makeNow(host, ask, pkg, component)
        } catch (e: Exception) {
            Log.w(TAG, "window ${ask.id} was not made", e)
            AppOpenRule.FAILED
        }
        making.remove(ask.id)
        done(result)
        if (result != AppOpenRule.OPENED && isEmpty) {
            ShizukuBridge.release()
            onEmpty()
        }
    }

    private fun makeNow(host: IWindowHost?, ask: WindowRule.Ask, pkg: String, component: String): String {
        if (host == null || closing) return AppOpenRule.WINDOWS_UNAVAILABLE
        val encoder = CastEncoder(ask.width, ask.height, { close(ask.id) }, window = ask.id, input = true)
        val displayId = host.createDisplay("Pocketdesk window ${ask.id}", ask.width, ask.height, ask.dpi, encoder.surface)
        if (displayId < 0) {
            encoder.release()
            return AppOpenRule.WINDOWS_UNAVAILABLE
        }
        // Everything was given back while the display was being made: it goes too.
        if (closing) {
            runCatching { host.release(displayId) }
            encoder.release()
            return AppOpenRule.WINDOWS_UNAVAILABLE
        }
        windows[ask.id] = Window(ask.id, pkg, encoder, displayId, ask.width, ask.height)
        if (!host.launch(displayId, component)) {
            close(ask.id)
            return AppOpenRule.FAILED
        }
        Log.i(TAG, "window ${ask.id} is open on display $displayId")
        return AppOpenRule.OPENED
    }

    /** One event of the computer's input to window [id]; a close ends it. Never on the main thread. */
    fun input(id: Int, event: WindowInput.Event) {
        val window = windows[id] ?: return
        if (event is WindowInput.Event.Close) return close(id)
        val host = runCatching { ShizukuBridge.currentHost() }.getOrNull() ?: return
        runCatching {
            when (event) {
                is WindowInput.Event.Touch -> finger(host, window, 0, event.action, event.x, event.y)
                is WindowInput.Event.Pointer -> finger(host, window, event.pointer, event.action, event.x, event.y)
                is WindowInput.Event.Scroll -> host.injectScroll(
                    window.displayId,
                    WindowInput.pixel(event.x, window.width),
                    WindowInput.pixel(event.y, window.height),
                    WindowInput.verticalScroll(event.notches),
                )
                is WindowInput.Event.Key -> host.injectKey(window.displayId, event.code)
                is WindowInput.Event.Text -> host.injectText(window.displayId, event.text)
                WindowInput.Event.Close -> Unit
            }
        }.onFailure { Log.w(TAG, "input to window $id failed", it) }
    }

    /** One finger's change in [window], injected with every finger then down (one gesture). */
    private fun finger(host: IWindowHost, window: Window, pointer: Int, action: Int, x: Int, y: Int) {
        val frame = synchronized(window.fingers) {
            window.fingers.apply(pointer, action, x, y, SystemClock.uptimeMillis())
        } ?: return
        host.injectPointers(
            window.displayId,
            frame.action,
            frame.ids,
            FloatArray(frame.ids.size) { WindowInput.pixel(frame.xs[it], window.width) },
            FloatArray(frame.ids.size) { WindowInput.pixel(frame.ys[it], window.height) },
            frame.downTime,
        )
    }

    /** Close window [id]: its display is given back (the app on it ends), its encoder freed, the computer told. */
    fun close(id: Int) {
        val window = windows.remove(id) ?: return
        Thread({
            runCatching { ShizukuBridge.currentHost()?.release(window.displayId) }
            window.encoder.release()
            runCatching { RustCore.castWindowClosed(id) }
            Log.i(TAG, "window $id is closed")
            if (isEmpty) {
                ShizukuBridge.release()
                onEmpty()
            }
        }, "window-close").start()
    }

    /** Close every window, and let the helper process go. Called when the cast ends or windows are turned off. */
    fun closeAll() {
        closing = true
        val all = windows.values.toList()
        windows.clear()
        Thread({
            all.forEach { window ->
                runCatching { ShizukuBridge.currentHost()?.release(window.displayId) }
                window.encoder.release()
                // The computer is told each one ended, so its window closes with it.
                runCatching { RustCore.castWindowClosed(window.id) }
            }
            ShizukuBridge.release()
        }, "window-close-all").start()
    }

    private companion object {
        const val TAG = "WindowCast"

        /** What the phone's settings, application management, install and permission screens resolve to. */
        fun deniedPackages(context: Context): Set<String> {
            val pm = context.packageManager
            val intents = listOf(
                Intent(android.provider.Settings.ACTION_SETTINGS),
                Intent(android.provider.Settings.ACTION_MANAGE_APPLICATIONS_SETTINGS),
                Intent(android.provider.Settings.ACTION_SECURITY_SETTINGS),
                Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS),
                Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, android.net.Uri.parse("package:x.y")),
                Intent(Intent.ACTION_INSTALL_PACKAGE).setDataAndType(android.net.Uri.parse("content://x/y.apk"), "application/vnd.android.package-archive"),
                Intent(Intent.ACTION_UNINSTALL_PACKAGE, android.net.Uri.parse("package:x.y")),
            )
            val found = HashSet<String>()
            for (intent in intents) {
                runCatching {
                    pm.queryIntentActivities(intent, 0).forEach { found.add(it.activityInfo.packageName) }
                }
            }
            // The permission controller, by the platform's own name for it, where it can be asked.
            runCatching {
                (PackageManager::class.java.getMethod("getPermissionControllerPackageName").invoke(pm) as? String)
                    ?.let { found.add(it) }
            }
            return found
        }
    }
}
