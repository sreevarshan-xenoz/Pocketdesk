package dev.pocketdesk.android.location

import android.Manifest
import android.app.ActivityManager
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.core.location.LocationManagerCompat
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.files.PhoneFilesRule
import dev.pocketdesk.android.notify.PhoneLink
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Settings "Give my computer my location" (GeoClue): off by default, and so is the exact choice under it. */
object PhoneLocationPrefs {
    private const val PREFS = "pocketdesk-phone-location"
    private const val KEY_ON = "on"
    private const val KEY_EXACT = "exact"

    const val COARSE = Manifest.permission.ACCESS_COARSE_LOCATION
    const val FINE = Manifest.permission.ACCESS_FINE_LOCATION
    const val BACKGROUND = Manifest.permission.ACCESS_BACKGROUND_LOCATION

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun setOn(ctx: Context, value: Boolean) = prefs(ctx).edit {
        putBoolean(KEY_ON, value)
        // Turned off, Exact goes too: on again starts at the city (review).
        if (!value) putBoolean(KEY_EXACT, false)
    }

    /** The owner lets the computer have an exact place, not just the city: off by default. */
    fun exact(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_EXACT, false)

    fun setExact(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_EXACT, value) }

    private fun granted(ctx: Context, permission: String) =
        ContextCompat.checkSelfPermission(ctx, permission) == PackageManager.PERMISSION_GRANTED

    fun coarseGranted(ctx: Context): Boolean = granted(ctx, COARSE)

    fun fineGranted(ctx: Context): Boolean = granted(ctx, FINE)

    /** "Allow all the time": where Android has one (10 and later), and it is granted. */
    fun backgroundGranted(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || granted(ctx, BACKGROUND)
}

/**
 * The computer asking where this phone is (GeoClue), run on the notification
 * listener's tick: one fix, from `getCurrentLocation`, at the precision the
 * computer asked for and this phone's owner allows (about a kilometre unless
 * both say exact), sent back on the link to the computer that asked. Only
 * this phone's own computer is answered, with the switch on, and no more
 * often than every 30 seconds.
 *
 * Nothing is stored and nothing is logged but "fix sent, accuracy N m".
 */
class PhoneLocation(private val ctx: Context) {
    private data class Request(val host: String, val port: Int, val precision: PhoneLocationRule.Precision)

    private val worker = Executors.newSingleThreadExecutor()
    private val inFlight = AtomicBoolean(false)

    @Volatile
    private var lastAnswerMs = 0L

    private var lastConnectMs = 0L

    fun tick() {
        val requests = takeRequests()
        val on = PhoneLocationPrefs.on(ctx)
        if (!on && requests.isEmpty()) return
        val own = PhoneLink.target(ctx)
        // The computer's asks come on the control link: kept while on.
        if (on && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        for (request in requests) {
            val ownComputer = PhoneFilesRule.fromOwnComputer(request.host, request.port, own?.address, own?.controlPort)
            if (!ownComputer) {
                Log.i(TAG, "a location ask from a computer that is not this phone's own was ignored")
                continue
            }
            answer(request)
        }
    }

    private fun takeRequests(): List<Request> =
        runCatching {
            val array = JSONArray(RustCore.takeLocationRequests())
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Request(o.getString("host"), o.getInt("port"), PhoneLocationRule.Precision.of(o.optString("precision")))
            }
        }.getOrDefault(emptyList())

    private fun send(request: Request, body: JSONObject) {
        RustCore.sendPhonePacket(request.host, request.port, FIX, body.toString())
            .onFailure { Log.w(TAG, "could not answer the computer's location ask") }
    }

    private fun refuse(request: Request, why: String) {
        Log.i(TAG, "location ask answered: $why")
        send(request, JSONObject().put("error", why))
    }

    private fun answer(request: Request) {
        val why =
            PhoneLocationRule.refusal(
                PhoneLocationPrefs.on(ctx),
                PhoneLocationPrefs.coarseGranted(ctx),
                PhoneLocationPrefs.fineGranted(ctx),
            )
        if (why != null) {
            refuse(request, why)
            return
        }
        val now = SystemClock.elapsedRealtime()
        if (!PhoneLocationRule.mayAnswer(lastAnswerMs, now)) {
            // Too soon after the last: the computer waits a minute and asks again.
            refuse(request, "unavailable")
            return
        }
        if (!inFlight.compareAndSet(false, true)) return
        val exact =
            PhoneLocationRule.exactAllowed(PhoneLocationPrefs.exact(ctx), PhoneLocationPrefs.fineGranted(ctx))
        val precision = PhoneLocationRule.heldTo(request.precision, exact)
        fetch(precision) { location ->
            try {
                reply(request, precision, location)
            } finally {
                inFlight.set(false)
            }
        }
    }

    private fun reply(request: Request, precision: PhoneLocationRule.Precision, location: Location?) {
        if (location == null) {
            refuse(request, PhoneLocationRule.noFix(PhoneLocationPrefs.backgroundGranted(ctx), onScreen()))
            return
        }
        val fix =
            PhoneLocationRule.fixFor(location.latitude, location.longitude, location.accuracy, precision)
        if (fix == null) {
            refuse(request, "unavailable")
            return
        }
        lastAnswerMs = SystemClock.elapsedRealtime()
        send(
            request,
            JSONObject()
                .put("lat", fix.lat)
                .put("lon", fix.lon)
                .put("accuracy", fix.accuracyM)
                .put("precision", fix.precision.wire),
        )
        Log.i(TAG, "location fix sent, accuracy ${fix.accuracyM} m")
    }

    /** One reading from the system, or null when none came in [FIX_WITHIN_MS] or none can be had. */
    private fun fetch(precision: PhoneLocationRule.Precision, done: (Location?) -> Unit) {
        val manager = ctx.getSystemService(LocationManager::class.java)
        val provider = manager?.let { provider(it, precision) }
        if (manager == null || provider == null || !LocationManagerCompat.isLocationEnabled(manager)) {
            worker.execute { done(null) }
            return
        }
        val cancel = CancellationSignal()
        Handler(Looper.getMainLooper()).postDelayed({ cancel.cancel() }, FIX_WITHIN_MS)
        runCatching {
            @Suppress("MissingPermission")
            LocationManagerCompat.getCurrentLocation(manager, provider, cancel, worker) { done(it) }
        }.onFailure {
            Log.w(TAG, "the system would not give a location", it)
            worker.execute { done(null) }
        }
    }

    /** GPS only for an exact fix (fine permission already checked); the network's reading otherwise. */
    private fun provider(manager: LocationManager, precision: PhoneLocationRule.Precision): String? {
        val candidates =
            if (precision == PhoneLocationRule.Precision.EXACT) {
                listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER)
            } else {
                listOf(LocationManager.NETWORK_PROVIDER, LocationManager.GPS_PROVIDER)
            }
        return candidates.firstOrNull { name ->
            // Coarse permission cannot use GPS.
            (name != LocationManager.GPS_PROVIDER || PhoneLocationPrefs.fineGranted(ctx)) &&
                runCatching { manager.isProviderEnabled(name) }.getOrDefault(false)
        }
    }

    private fun onScreen(): Boolean {
        val info = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(info)
        return info.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    fun close() {
        worker.shutdownNow()
    }

    companion object {
        private const val TAG = "PhoneLocation"
        private const val RECONNECT_MS = 30_000L
        private const val FIX_WITHIN_MS = 20_000L

        /** Phone → computer: the answer to an ask. */
        const val FIX = "kdeconnect.pocketdesk.phone_location"
    }
}
