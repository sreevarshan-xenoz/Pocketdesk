package dev.pocketdesk.android.pair

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiNetworkSpecifier
import android.os.Build
import androidx.annotation.RequiresApi
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/**
 * What a scanned pairing link asks of the phone's Wi-Fi: nothing (the usual
 * link), a join of the computer's own network (a direct link), or a refusal
 * to join one. Held apart from the network calls so a test reads it.
 */
enum class DirectLinkDecision {
    /** The link names no Wi-Fi: pair over whatever network the phone is on. */
    NotADirectLink,

    /** Join the computer's own network first, for Pocketdesk only. */
    Join,

    /** The owner turned "Join computers' direct links" off. */
    Disabled,

    /** The system's per-app network request needs Android 10. */
    TooOld,
    ;

    companion object {
        /** `WifiNetworkSpecifier` arrived with Android 10 (API 29). */
        const val MIN_SDK = 29

        fun decide(wifi: WifiJoin?, joinEnabled: Boolean, sdk: Int): DirectLinkDecision = when {
            wifi == null -> NotADirectLink
            !joinEnabled -> Disabled
            sdk < MIN_SDK -> TooOld
            else -> Join
        }
    }
}

/** How a join of a computer's own network ended. */
sealed interface JoinOutcome {
    data class Joined(val network: Network) : JoinOutcome

    /** Not joined: declined in the system's prompt, not found, or timed out. */
    data object Unavailable : JoinOutcome
}

/**
 * The computer's own network a direct link names, joined as an app-bound
 * network: the system asks the owner once, the phone keeps its mobile data
 * and its other Wi-Fi for everything else, and Pocketdesk's own sockets (the
 * control link, the stream, files; all made by the Rust bridge in this
 * process) go to the computer through it.
 *
 * Every socket is bound by binding the process
 * ([ConnectivityManager.bindProcessToNetwork]) rather than socket by socket:
 * the bridge opens TCP and QUIC sockets from several places, and a process
 * binding marks each at creation, so none is missed. The cost is that
 * Pocketdesk's own internet traffic (it has little) waits for the link to be
 * released; the phone's other apps are untouched. It is released when the
 * session ends or the pairing is left ([release]).
 */
object DirectLinkNetwork {
    /** How long the system's prompt and the join may take. */
    private const val JOIN_TIMEOUT_MS = 60_000

    private var callback: ConnectivityManager.NetworkCallback? = null
    private var bound: Network? = null

    private fun manager(context: Context) =
        context.applicationContext.getSystemService(ConnectivityManager::class.java)

    /**
     * Join [wifi]'s network and bind this process to it. Call off nothing
     * special: it suspends while the system's prompt is up. Only one at a
     * time: an earlier join is released first.
     */
    suspend fun join(context: Context, wifi: WifiJoin): JoinOutcome {
        if (Build.VERSION.SDK_INT < DirectLinkDecision.MIN_SDK) return JoinOutcome.Unavailable
        return joinApi29(context, wifi)
    }

    @RequiresApi(DirectLinkDecision.MIN_SDK)
    private suspend fun joinApi29(context: Context, wifi: WifiJoin): JoinOutcome {
        val cm = manager(context)
        release(context)
        val specifier = WifiNetworkSpecifier.Builder()
            .setSsid(wifi.ssid)
            .setWpa2Passphrase(wifi.passphrase)
            .build()
        // No internet is promised: the computer's network leads nowhere else.
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .setNetworkSpecifier(specifier)
            .build()
        return suspendCancellableCoroutine { continuation ->
            val own = object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    synchronized(DirectLinkNetwork) {
                        // A callback that was released meanwhile binds nothing.
                        if (callback !== this) return
                        bound = network
                        cm.bindProcessToNetwork(network)
                    }
                    if (continuation.isActive) continuation.resume(JoinOutcome.Joined(network))
                }

                override fun onUnavailable() {
                    if (continuation.isActive) continuation.resume(JoinOutcome.Unavailable)
                }

                override fun onLost(network: Network) {
                    synchronized(DirectLinkNetwork) {
                        if (bound == network) {
                            bound = null
                            cm.bindProcessToNetwork(null)
                        }
                    }
                }
            }
            synchronized(this) { callback = own }
            continuation.invokeOnCancellation { release(context) }
            try {
                cm.requestNetwork(request, own, JOIN_TIMEOUT_MS)
            } catch (e: SecurityException) {
                // CHANGE_NETWORK_STATE is a normal permission; a ROM that
                // refuses it is the same as the owner declining.
                synchronized(this) { callback = null }
                if (continuation.isActive) continuation.resume(JoinOutcome.Unavailable)
            }
        }
    }

    /**
     * Let the phone's network be its own again: the process unbound, the
     * request withdrawn, which makes the system leave the computer's network.
     * Safe to call when nothing is joined.
     */
    fun release(context: Context) {
        val cm = manager(context)
        val old = synchronized(this) {
            val taken = callback
            callback = null
            if (bound != null) {
                bound = null
                cm.bindProcessToNetwork(null)
            }
            taken
        }
        if (old != null) {
            try {
                cm.unregisterNetworkCallback(old)
            } catch (_: IllegalArgumentException) {
                // Already unregistered.
            }
        }
    }
}
