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

    /** The helper process if it is up now, else null. */
    fun currentHost(): IWindowHost? = host?.takeIf { it.asBinder().pingBinder() }

    /** The helper process, started if need be; [onReady] gets it, or null when it could not start. */
    @Synchronized
    fun host(context: Context, onReady: (IWindowHost?) -> Unit) {
        host?.takeIf { it.asBinder().pingBinder() }?.let { return onReady(it) }
        if (!running() || !permitted()) return onReady(null)
        val service = object : android.content.ServiceConnection {
            override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
                val made = binder?.takeIf { it.pingBinder() }?.let { IWindowHost.Stub.asInterface(it) }
                host = made
                onReady(made)
            }

            override fun onServiceDisconnected(name: ComponentName?) {
                host = null
            }
        }
        val userService = Shizuku.UserServiceArgs(ComponentName(context.packageName, WindowHostService::class.java.name))
            .processNameSuffix("windows")
            .daemon(false)
            .version(1)
        args = userService
        connection = service
        runCatching { Shizuku.bindUserService(userService, service) }.onFailure {
            Log.w(TAG, "the helper process did not start", it)
            onReady(null)
        }
    }

    /** Let the helper process go: no window is open. */
    @Synchronized
    fun release() {
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
