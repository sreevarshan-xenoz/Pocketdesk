package dev.pocketdesk.android.windows

import android.annotation.SuppressLint
import android.content.Context
import android.os.IBinder
import android.util.Log
import java.lang.reflect.Proxy
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.function.Supplier

/**
 * Let my computers turn on my hotspot: what the Shizuku helper process (shell
 * identity) does to start and stop the phone's own Wi-Fi hotspot. It takes no
 * argument from anyone: the hotspot starts with the settings Android already
 * holds for it, so no name, password or band passes through here.
 *
 * Starting asks Android's tethering manager (the system's own service, the
 * one Settings' hotspot switch uses) to start Wi-Fi tethering, with no
 * provisioning screen. Stopping asks it to stop, and if it does not answer,
 * runs [HotspotRule.STOP_ARGV], fixed words with no shell. Everything is
 * reflection on hidden Android APIs, which a shell process may use; each step
 * answers false rather than throwing.
 *
 * Not verified on a device yet: see the owed check in the device checklist.
 */
@SuppressLint("PrivateApi", "DiscouragedPrivateApi")
internal object HotspotShell {
    private const val TAG = "HotspotShell"

    /** `TetheringManager.TETHERING_WIFI`. */
    private const val TETHERING_WIFI = 0
    private const val START_WAIT_S = 15L
    private const val ASK_WAIT_S = 3L
    private const val STOP_WAIT_S = 10L

    /** The tethering manager, made for the shell's own package so the system's checks name uid 2000. */
    private fun manager(context: Context): Any? = runCatching {
        val clazz = Class.forName("android.net.TetheringManager")
        val binder = Class.forName("android.os.ServiceManager")
            .getMethod("getService", String::class.java)
            .invoke(null, "tethering") as? IBinder
        if (binder != null) {
            clazz.getDeclaredConstructor(Context::class.java, Supplier::class.java)
                .apply { isAccessible = true }
                .newInstance(context, Supplier { binder })
        } else {
            context.getSystemService("tethering")
        }
    }.onFailure { Log.w(TAG, "no tethering manager", it) }.getOrNull()

    /**
     * Whether a Wi-Fi access point is being tethered now: the tethering manager tells a new listener the
     * current interfaces at once. Null when that could not be asked. USB and Bluetooth tethers do not count.
     */
    private fun wifiHotspotRunning(manager: Any): Boolean? = runCatching {
        val clazz = Class.forName("android.net.TetheringManager")
        val callbackClass = Class.forName("android.net.TetheringManager\$TetheringEventCallback")
        val heard = CountDownLatch(1)
        var tethered: List<*>? = null
        val callback = Proxy.newProxyInstance(callbackClass.classLoader, arrayOf(callbackClass)) { proxy, method, args ->
            when (method.name) {
                "onTetheredInterfacesChanged" -> {
                    tethered = args?.firstOrNull() as? List<*>
                    heard.countDown()
                }
                "hashCode" -> System.identityHashCode(proxy)
                "equals" -> proxy === args?.firstOrNull()
                "toString" -> "TetheringEventCallback"
                else -> null
            }
        }
        clazz.getMethod("registerTetheringEventCallback", Executor::class.java, callbackClass)
            .invoke(manager, Executor { it.run() }, callback)
        val answered = heard.await(ASK_WAIT_S, TimeUnit.SECONDS)
        runCatching { clazz.getMethod("unregisterTetheringEventCallback", callbackClass).invoke(manager, callback) }
        if (answered) HotspotRule.wifiApAmong(tethered.orEmpty().map { it.toString() }) else null
    }.onFailure { Log.w(TAG, "could not ask whether a hotspot is running", it) }.getOrNull()

    /**
     * Start Wi-Fi tethering, waiting for the system's answer: [HotspotRule.RESULT_DONE] when it started,
     * [HotspotRule.RESULT_ALREADY_ON] when one was running already (nothing is started), else
     * [HotspotRule.RESULT_FAILED]. When whether one runs cannot be asked, it is started as before.
     */
    fun start(context: Context): Int = runCatching {
        val manager = manager(context) ?: return HotspotRule.RESULT_FAILED
        if (wifiHotspotRunning(manager) == true) return HotspotRule.RESULT_ALREADY_ON
        val clazz = Class.forName("android.net.TetheringManager")
        val requestClass = Class.forName("android.net.TetheringManager\$TetheringRequest")
        val builderClass = Class.forName("android.net.TetheringManager\$TetheringRequest\$Builder")
        val callbackClass = Class.forName("android.net.TetheringManager\$StartTetheringCallback")
        val builder = builderClass.getConstructor(Int::class.javaPrimitiveType).newInstance(TETHERING_WIFI)
        // No provisioning screen: nothing may appear on the phone for it to wait on.
        runCatching {
            builderClass.getMethod("setShouldShowEntitlementUi", Boolean::class.javaPrimitiveType).invoke(builder, false)
        }
        val request = builderClass.getMethod("build").invoke(builder)
        val done = CountDownLatch(1)
        var started = false
        val callback = Proxy.newProxyInstance(callbackClass.classLoader, arrayOf(callbackClass)) { proxy, method, args ->
            when (method.name) {
                "onTetheringStarted" -> {
                    started = true
                    done.countDown()
                }
                "onTetheringFailed" -> {
                    Log.w(TAG, "tethering did not start: ${args?.firstOrNull()}")
                    done.countDown()
                }
                "hashCode" -> System.identityHashCode(proxy)
                "equals" -> proxy === args?.firstOrNull()
                "toString" -> "StartTetheringCallback"
                else -> null
            }
        }
        clazz.getMethod("startTethering", requestClass, Executor::class.java, callbackClass)
            .invoke(manager, request, Executor { it.run() }, callback)
        if (done.await(START_WAIT_S, TimeUnit.SECONDS) && started) HotspotRule.RESULT_DONE else HotspotRule.RESULT_FAILED
    }.onFailure { Log.w(TAG, "the hotspot did not start", it) }.getOrDefault(HotspotRule.RESULT_FAILED)

    /** Stop Wi-Fi tethering: the manager first, then the fixed command. True when one of them went through. */
    fun stop(context: Context): Boolean {
        val asked = runCatching {
            val manager = manager(context) ?: return@runCatching false
            Class.forName("android.net.TetheringManager")
                .getMethod("stopTethering", Int::class.javaPrimitiveType)
                .invoke(manager, TETHERING_WIFI)
            true
        }.onFailure { Log.w(TAG, "the manager did not stop the hotspot", it) }.getOrDefault(false)
        return asked || runStopCommand()
    }

    /** [HotspotRule.STOP_ARGV], with a limit; true when it exits 0. */
    private fun runStopCommand(): Boolean = runCatching {
        val process = ProcessBuilder(HotspotRule.STOP_ARGV).redirectErrorStream(true).start()
        if (!process.waitFor(STOP_WAIT_S, TimeUnit.SECONDS)) {
            process.destroy()
            return false
        }
        process.exitValue() == 0
    }.onFailure { Log.w(TAG, "the stop command did not run", it) }.getOrDefault(false)
}
