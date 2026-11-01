package dev.pocketdesk.android.windows

import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.os.IBinder
import android.util.Log
import rikka.shizuku.Shizuku

/**
 * Phone apps tier 2 (separate windows): the one place Shizuku's API is
 * touched. Optional: nothing here is called until the owner turns on
 * "Separate windows (Shizuku)", and every call answers "no" when Shizuku is
 * not there. Pocketdesk never asks anyone to install or start Shizuku.
 *
 * Shizuku shows its own permission dialog, which is the owner's grant; the
 * helper process it starts runs with shell identity, and Pocketdesk uses it
 * only through [IWindowHost]: make a display, start an app on it, send input
 * to it.
 */
object ShizukuBridge {
    private const val TAG = "ShizukuBridge"
    private const val REQUEST_CODE = 4716

    /** Whether Shizuku is running (its binder answers); false when it is not installed or not started. */
    fun running(): Boolean = runCatching { Shizuku.pingBinder() }.getOrDefault(false)

    /** Whether this app holds Shizuku's permission. */
    fun permitted(): Boolean = runCatching {
        Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    }.getOrDefault(false)

    /**
     * Ask Shizuku's own dialog for its permission, once; [onResult] says
     * whether it was given. Answers false at once when Shizuku is not
     * running or the owner already refused it for good.
     */
    fun request(onResult: (Boolean) -> Unit) {
        if (!running()) return onResult(false)
        if (permitted()) return onResult(true)
        val denied = runCatching { Shizuku.shouldShowRequestPermissionRationale() }.getOrDefault(true)
        if (denied) return onResult(false)
        val listener = object : Shizuku.OnRequestPermissionResultListener {
            override fun onRequestPermissionResult(requestCode: Int, grantResult: Int) {
                if (requestCode != REQUEST_CODE) return
                Shizuku.removeRequestPermissionResultListener(this)
                onResult(grantResult == PackageManager.PERMISSION_GRANTED)
            }
        }
        runCatching {
            Shizuku.addRequestPermissionResultListener(listener)
            Shizuku.requestPermission(REQUEST_CODE)
        }.onFailure {
            Log.w(TAG, "Shizuku's dialog could not be asked", it)
            runCatching { Shizuku.removeRequestPermissionResultListener(listener) }
            onResult(false)
        }
    }

    @Volatile
    private var host: IWindowHost? = null
    private var args: Shizuku.UserServiceArgs? = null
    private var connection: android.content.ServiceConnection? = null

    /** Told when the helper process dies, so the windows forget the fingers they thought were down. */
    @Volatile
    var onLost: (() -> Unit)? = null

    /** The helper process if it is up now, else null. */
    fun currentHost(): IWindowHost? = host?.takeIf { it.asBinder().pingBinder() }

    /** Askers waiting for the helper process while it starts; one bind serves them all. */
    private val waiting = ArrayList<(IWindowHost?) -> Unit>()
    private var binding = false

    /** Hand [made] (or null) to everyone waiting. */
    private fun answer(made: IWindowHost?) {
        val askers = synchronized(this) {
            binding = false
            host = made
            ArrayList(waiting).also { waiting.clear() }
        }
        askers.forEach { it(made) }
    }

    /** The helper process, started if need be; [onReady] gets it, or null when it could not start. */
    fun host(context: Context, onReady: (IWindowHost?) -> Unit) {
        val bind = synchronized(this) {
            host?.takeIf { it.asBinder().pingBinder() }?.let { up ->
                // Up already: answered outside the lock.
                return@synchronized up
            }
            if (!running() || !permitted()) return@synchronized false
            waiting.add(onReady)
            // One bind at a time: a second ask while one is under way only waits for it.
            if (binding) return@synchronized null
            binding = true
            true
        }
        when (bind) {
            is IWindowHost -> return onReady(bind)
            false -> return onReady(null)
            null -> return
            else -> Unit
        }
        val service = object : android.content.ServiceConnection {
            override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
                answer(binder?.takeIf { it.pingBinder() }?.let { IWindowHost.Stub.asInterface(it) })
            }

            override fun onServiceDisconnected(name: ComponentName?) {
                synchronized(this@ShizukuBridge) { host = null }
                // The helper's displays went with it: no finger is down on them any more.
                onLost?.invoke()
            }
        }
        val userService = Shizuku.UserServiceArgs(ComponentName(context.packageName, WindowHostService::class.java.name))
            .processNameSuffix("windows")
            .daemon(false)
            // 3: the helper gained the hotspot calls (transactions 9 and 10).
            .version(3)
        synchronized(this) {
            args = userService
            connection = service
        }
        runCatching { Shizuku.bindUserService(userService, service) }.onFailure {
            Log.w(TAG, "the helper process did not start", it)
            answer(null)
        }
    }

    /** Hotspot jobs under way: [release] leaves the helper alone while one is, and the job lets it go after. */
    private val hotspotJobs = java.util.concurrent.atomic.AtomicInteger(0)

    /**
     * Let my computers turn on my hotspot: have the helper process turn the hotspot on or off
     * ([on]), starting it if need be, off the caller's thread; [done] gets whether it went
     * through. [windowsOpen] says whether app windows still use the helper, which is let go
     * after the job only when none does.
     */
    fun hotspot(context: Context, on: Boolean, windowsOpen: () -> Boolean, done: (Int) -> Unit) {
        hotspotJobs.incrementAndGet()
        val finished = java.util.concurrent.atomic.AtomicBoolean(false)
        // Once, whichever comes first: the helper's answer or the limit on waiting for it.
        fun finish(result: Int) {
            if (!finished.compareAndSet(false, true)) return
            if (hotspotJobs.decrementAndGet() == 0 && !windowsOpen()) release()
            done(result)
        }
        val onReady: (IWindowHost?) -> Unit = { helper ->
            Thread {
                val result = runCatching {
                    when {
                        helper == null -> HotspotRule.RESULT_FAILED
                        on -> helper.startHotspot()
                        else -> if (helper.stopHotspot()) HotspotRule.RESULT_DONE else HotspotRule.RESULT_FAILED
                    }
                }.onFailure { Log.w(TAG, "the hotspot call failed", it) }.getOrDefault(HotspotRule.RESULT_FAILED)
                finish(result)
            }.start()
        }
        // A helper that never answers its bind must not hold this job (and its callers' flags) for ever.
        Thread {
            if (!finished.get()) {
                Thread.sleep(BIND_LIMIT_MS)
                if (!finished.get()) {
                    Log.w(TAG, "the helper did not answer in time for a hotspot ask")
                    abandon(onReady)
                    finish(HotspotRule.RESULT_FAILED)
                }
            }
        }.apply { isDaemon = true }.start()
        host(context, onReady)
    }

    /** How long a hotspot ask waits for the helper, bind and call together. */
    private const val BIND_LIMIT_MS = 45_000L

    /** Stop waiting for [onReady]; a bind nobody waits for any more can be started again. */
    private fun abandon(onReady: (IWindowHost?) -> Unit) = synchronized(this) {
        waiting.remove(onReady)
        if (waiting.isEmpty()) binding = false
    }

    /** Let the helper process go: no window is open. */
    @Synchronized
    fun release() {
        // A hotspot job still using it lets it go itself when it is done.
        if (hotspotJobs.get() > 0) return
        val made = host
        host = null
        runCatching { made?.destroy() }
        val userService = args
        val service = connection
        args = null
        connection = null
        if (userService != null && service != null) {
            runCatching { Shizuku.unbindUserService(userService, service, true) }
        }
    }
}
