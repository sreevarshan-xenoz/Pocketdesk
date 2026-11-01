package dev.pocketdesk.android.service

import android.app.ForegroundServiceStartNotAllowedException
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.wake.WakeHelperKeeper
import dev.pocketdesk.android.wake.WakeHelperRule
import dev.pocketdesk.android.wake.Waker
import org.json.JSONArray
import org.json.JSONObject

/**
 * A spare phone at home as the wake helper. While "Wake helper for this home"
 * is on, this service is in the foreground; the listener (an iroh endpoint on
 * its own key, in the bridge) runs only while the phone is on a charger and on
 * Wi-Fi ([WakeHelperRule.state]). It stops listening, and the service says
 * so in its notification, the moment the phone is unplugged or leaves Wi-Fi.
 *
 * It serves one request: send this computer's magic packet, for a key the
 * computer introduced, at most once every 30 seconds. The MAC and the address
 * come from the computer's own word ([WakeHelperKeeper]), never from the
 * request. It needs no permission the app does not already hold: type
 * `connectedDevice`, as the link's service has, and the battery and network
 * state any app may read.
 *
 * Idle, on the battery, it holds no timer and no wake lock: only the
 * receivers for power and network changes.
 */
class WakeHelperService : Service() {
    companion object {
        private const val TAG = "WakeHelperService"
        private const val CHANNEL_ID = "pocketdesk-wake-helper"
        private const val NOTIFICATION_ID = 41_100

        /** How long a start Android refused waits before the next try. */
        private const val RETRY_MS = 60_000L

        /** How long the link to the computer is left alone after an attempt. */
        private const val RELINK_MS = 30_000L

        @Volatile
        private var running = false

        /** Whether the service is in the foreground now. */
        fun isRunning(): Boolean = running

        @Volatile
        private var refusedAt = 0L

        @Volatile
        private var warned = false

        /** Whether the phone is on a charger now. */
        fun charging(ctx: Context): Boolean {
            val status = ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            return (status?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0
        }

        /** Whether the phone is on a Wi-Fi or Ethernet network that reaches the internet. */
        @Suppress("DEPRECATION")
        fun onWifi(ctx: Context): Boolean {
            val cm = ctx.getSystemService(ConnectivityManager::class.java) ?: return false
            return cm.allNetworks.any { network ->
                val caps = cm.getNetworkCapabilities(network) ?: return@any false
                caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                    (
                        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
                        )
            }
        }

        /** What the phone is doing as a helper now. */
        fun state(ctx: Context): WakeHelperRule.State =
            WakeHelperRule.state(Prefs.wakeHelper(ctx), charging(ctx), onWifi(ctx))

        private fun text(ctx: Context): String = ctx.getString(
            if (state(ctx) == WakeHelperRule.State.SERVING) R.string.wake_helper_serving else R.string.wake_helper_waiting,
        )

        private fun build(ctx: Context): Notification {
            val open = PendingIntent.getActivity(
                ctx,
                0,
                Intent(ctx, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            return Notification.Builder(ctx, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_stat_pocketdesk)
                .setContentTitle(ctx.getString(R.string.wake_helper_title))
                .setContentText(text(ctx))
                .setContentIntent(open)
                .setOngoing(true)
                .build()
        }

        /**
         * The owner turned the switch off: tell the computer, best effort, so it
         * stops listing this phone as its helper, and forget what it said to do.
         */
        fun turnedOff(ctx: Context) {
            val app = ctx.applicationContext
            PhoneLink.worker.execute {
                PhoneLink.sendNow(app, WakeHelperRule.OFFER, JSONObject().put("on", false))
                WakeHelperKeeper.forgetDuties(app)
            }
        }

        /**
         * Start or stop the service as the switch says. Android may refuse a
         * start from the background; the next start from the app, a boot or an
         * update takes over.
         */
        fun sync(ctx: Context) {
            val app = ctx.applicationContext
            val wanted = WakeHelperRule.wanted(Prefs.wakeHelper(app))
            if (wanted == running) return
            if (wanted) {
                val now = SystemClock.elapsedRealtime()
                if (refusedAt != 0L && now - refusedAt < RETRY_MS) return
                try {
                    app.startForegroundService(Intent(app, WakeHelperService::class.java))
                } catch (e: Exception) {
                    val refused = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                        e is ForegroundServiceStartNotAllowedException
                    val line = if (refused) "Android refused to start it from the background" else "Could not start it: $e"
                    if (warned) Log.d(TAG, line) else Log.w(TAG, line)
                    warned = true
                    refusedAt = now
                }
            } else {
                app.stopService(Intent(app, WakeHelperService::class.java))
            }
        }
    }

    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    private var listening = false
    private var lastLinkAt = 0L
    private var lastOfferAt: Long? = null
    private var lastOfferKey: String? = null
    private var registered = false

    private val power = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            handler?.post { evaluate() }
        }
    }

    private val network = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            handler?.post { evaluate() }
        }

        override fun onLost(network: Network) {
            handler?.post { evaluate() }
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            handler?.post { evaluate() }
        }
    }

    private val tick = Runnable { tick() }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, build(this), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                startForeground(NOTIFICATION_ID, build(this))
            }
        } catch (e: Exception) {
            Log.w(TAG, "Could not go to the foreground: $e")
            refusedAt = SystemClock.elapsedRealtime()
            stopSelf()
            return START_NOT_STICKY
        }
        if (!WakeHelperRule.wanted(Prefs.wakeHelper(this))) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        refusedAt = 0L
        if (thread == null) {
            thread = HandlerThread("wake-helper").also { it.start() }
            handler = Handler(thread!!.looper)
        }
        if (!registered) {
            registered = true
            val filter = IntentFilter().apply {
                addAction(Intent.ACTION_POWER_CONNECTED)
                addAction(Intent.ACTION_POWER_DISCONNECTED)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                registerReceiver(power, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                @Suppress("UnspecifiedRegisterReceiverFlag")
                registerReceiver(power, filter)
            }
            getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(network)
        }
        handler?.post { evaluate() }
        // Not sticky: a restart in a new process is not a start Android
        // excuses; the app, a boot and an update start it again.
        return START_NOT_STICKY
    }

    /** Look at the switch, the charger and the network, and listen or not. */
    private fun evaluate() {
        val on = Prefs.wakeHelper(this)
        if (!on) {
            stopListening()
            WakeHelperKeeper.forgetDuties(this)
            handler?.post {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
            return
        }
        val serving = state(this) == WakeHelperRule.State.SERVING
        if (serving && !listening) startListening()
        if (!serving && listening) stopListening()
        runCatching {
            (getSystemService(NOTIFICATION_SERVICE) as NotificationManager).notify(NOTIFICATION_ID, build(this))
        }
    }

    private fun startListening() {
        val started = RustCore.wakeHelperStart()
        if (started.isFailure) {
            Log.w(TAG, "the listener did not start: ${started.exceptionOrNull()?.message}")
            return
        }
        listening = true
        Log.i(TAG, "listening")
        tick()
    }

    private fun stopListening() {
        handler?.removeCallbacks(tick)
        if (!listening) return
        listening = false
        RustCore.wakeHelperStop()
        Log.i(TAG, "not listening")
    }

    /** One pass while listening: keep what the computer said, hold the link, tell it who this is. */
    private fun tick() {
        if (!listening) return
        runCatching { pass() }.onFailure { Log.w(TAG, "a pass failed: ${it.message}") }
        if (listening) handler?.postDelayed(tick, WakeHelperRule.TICK_MS)
    }

    private fun pass() {
        if (WakeHelperKeeper.poll(this)) {
            // A computer removed this phone: the switch is off now.
            handler?.post { evaluate() }
            return
        }
        val own = PhoneLink.target(this)
        // The computer's words come on the control link: kept while listening.
        if (own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastLinkAt >= RELINK_MS) {
                lastLinkAt = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(this, own.address))
            }
        }
        val duties = JSONArray()
        for (duty in WakeHelperKeeper.duties(this)) runCatching { duties.put(JSONObject(duty)) }
        RustCore.wakeHelperSetDuties(duties.toString())
        RustCore.wakeHelperSetNetworks(JSONArray(Waker.phoneBroadcasts().toList()).toString())
        offer()
    }

    /**
     * Tell the computer this phone is its helper (the key it listens on and the
     * relays it is reached through), when that is new, changed or due again.
     */
    private fun offer() {
        val status = runCatching { JSONObject(RustCore.wakeHelperStatus()) }.getOrNull() ?: return
        val iroh = status.optString("id")
        val relays = status.optJSONArray("relays")?.let { a -> List(a.length()) { a.optString(it) } }.orEmpty()
        // Reached through a relay or not at all from outside: wait for one.
        if (iroh.isEmpty() || relays.isEmpty()) return
        val key = WakeHelperRule.offerKey(iroh, relays)
        val now = SystemClock.elapsedRealtime()
        if (!WakeHelperRule.offerDue(now, lastOfferAt, lastOfferKey, key)) return
        val fresh = Prefs.wakeHelperFresh(this)
        val body = JSONObject()
            .put("on", true)
            .put("fresh", fresh)
            .put("iroh", iroh)
            .put("relays", JSONArray(relays))
        if (PhoneLink.sendNow(applicationContext, WakeHelperRule.OFFER, body)) {
            lastOfferAt = now
            lastOfferKey = key
            if (fresh) Prefs.setWakeHelperFresh(this, false)
        }
    }

    override fun onDestroy() {
        running = false
        handler?.removeCallbacks(tick)
        if (listening) {
            listening = false
            RustCore.wakeHelperStop()
        }
        if (registered) {
            registered = false
            runCatching { unregisterReceiver(power) }
            runCatching { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(network) }
        }
        thread?.quitSafely()
        thread = null
        handler = null
        super.onDestroy()
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_wake_helper),
            NotificationManager.IMPORTANCE_MIN,
        )
        channel.setShowBadge(false)
        channel.description = getString(R.string.channel_wake_helper_desc)
        (getSystemService(NOTIFICATION_SERVICE) as NotificationManager).createNotificationChannel(channel)
    }
}
