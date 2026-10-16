package dev.pocketdesk.android.service

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.SystemClock
import dev.pocketdesk.android.doctor.ConnectionChecks

/**
 * The phone's networks as a transfer needs them (R6i): whether one it can
 * use is up ([TransferRetry.networkFits]), and a wait for one.
 */
internal object NetworkWait {
    /** How often a waiting transfer looks again. */
    private const val POLL_MS = 2_000L

    /** Once the network is back, a moment for its address and routes. */
    private const val SETTLE_MS = 3_000L

    /** Whether the phone has a network a transfer with the computer at [address] can use. */
    fun fits(context: Context, address: String): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return true
        @Suppress("DEPRECATION") // allNetworks: the Wi-Fi under a VPN has no other public lookup.
        val local = cm.allNetworks.any { network ->
            val caps = cm.getNetworkCapabilities(network) ?: return@any false
            !caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
                (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                    caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))
        } || sharing()
        return TransferRetry.networkFits(address, anyNetwork = cm.activeNetwork != null, localNetwork = local)
    }

    /** Whether this phone shares its connection (its hotspot or USB tethering), which a computer may be on. */
    private fun sharing(): Boolean =
        runCatching {
            java.net.NetworkInterface.getNetworkInterfaces().toList()
                .any { it.isUp && ConnectionChecks.sharingOf(it.name) != null }
        }.getOrDefault(false)

    /** Block up to [seconds] for [fits]; true once it does, after a moment to settle. */
    fun await(context: Context, address: String, seconds: Long): Boolean {
        val deadline = SystemClock.elapsedRealtime() + seconds * 1000
        while (SystemClock.elapsedRealtime() < deadline) {
            if (fits(context, address)) {
                Thread.sleep(SETTLE_MS)
                return true
            }
            Thread.sleep(POLL_MS)
        }
        return fits(context, address)
    }
}
