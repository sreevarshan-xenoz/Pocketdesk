package dev.pocketdesk.android.status

import android.app.usage.StorageStatsManager
import android.content.Context
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.StatFs
import android.os.storage.StorageManager
import android.telephony.SubscriptionManager
import android.telephony.TelephonyManager
import android.util.Log

/**
 * The phone's state now (L9), as [PhoneStateRule] says it: each part read
 * on its own, and left out when Android will not say. Without [full], only
 * the network's kind and the ringer, which decide a send.
 */
internal fun readPhoneState(context: Context, full: Boolean = true): PhoneStateRule.State {
    val storage = if (full) runCatching { storage(context) }.getOrNull() else null
    val (network, signal) = runCatching { network(context, full) }
        .onFailure { Log.d("PhoneState", "No network read: ${it.message}") }
        .getOrDefault(null to null)
    val ringer = runCatching {
        when (context.getSystemService(AudioManager::class.java)?.ringerMode) {
            AudioManager.RINGER_MODE_NORMAL -> PhoneStateRule.NORMAL
            AudioManager.RINGER_MODE_VIBRATE -> PhoneStateRule.VIBRATE
            AudioManager.RINGER_MODE_SILENT -> PhoneStateRule.SILENT
            else -> null
        }
    }.getOrNull()
    return PhoneStateRule.State(
        storageFreeMb = storage?.first,
        storageTotalMb = storage?.second,
        network = network,
        signal = signal,
        ringer = ringer,
    )
}

/**
 * Free and total storage as the phone's Settings says them (L9 review):
 * the internal volume's advertised size, not the data partition's.
 * [StatFs] stands in if that will not answer.
 */
private fun storage(context: Context): Pair<Long, Long>? {
    val stats = context.getSystemService(StorageStatsManager::class.java)
    val read = stats?.let {
        runCatching {
            it.getFreeBytes(StorageManager.UUID_DEFAULT) to it.getTotalBytes(StorageManager.UUID_DEFAULT)
        }.getOrNull()
    }
    val (free, total) = read ?: StatFs(context.filesDir.path).let { it.availableBytes to it.totalBytes }
    return PhoneStateRule.storage(free, total)
}

/** The default network's kind and, with [bars], its bars: never its name. */
private fun network(context: Context, bars: Boolean): Pair<String?, Int?> {
    val cm = context.getSystemService(ConnectivityManager::class.java) ?: return null to null
    val active = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) }
        ?: return PhoneStateRule.NONE to null
    // Behind a VPN (Tailscale, say) the network it rides (L9 review): the
    // VPN's own has no bars, and before Android 9 not its transport.
    val caps = if (active.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) underlying(cm) ?: active else active
    return when {
        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> {
            val wifiBars = if (bars && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                PhoneStateRule.wifiBars(caps.signalStrength)
            } else {
                null
            }
            PhoneStateRule.WIFI to wifiBars
        }
        caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> {
            val mobileBars = if (bars && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                // Needs no permission; a phone that refuses says no bars.
                // The SIM that carries data, on a phone with two (L9 review).
                runCatching {
                    val tm = context.getSystemService(TelephonyManager::class.java)
                    val data = SubscriptionManager.getDefaultDataSubscriptionId()
                    val forData = if (data != SubscriptionManager.INVALID_SUBSCRIPTION_ID) {
                        tm?.createForSubscriptionId(data)
                    } else {
                        tm
                    }
                    forData?.signalStrength?.level
                }.getOrNull()?.let(PhoneStateRule::mobileBars)
            } else {
                null
            }
            PhoneStateRule.MOBILE to mobileBars
        }
        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> PhoneStateRule.ETHERNET to null
        // A VPN alone, Bluetooth or USB tethering: none of the three.
        else -> null to null
    }
}

/** The network a VPN rides: one that is not a VPN, with the internet, validated first. */
@Suppress("DEPRECATION") // allNetworks: its replacement needs a callback kept
private fun underlying(cm: ConnectivityManager): NetworkCapabilities? {
    val all = cm.allNetworks.mapNotNull { cm.getNetworkCapabilities(it) }.filter {
        !it.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
            it.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }
    return all.firstOrNull { it.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) } ?: all.firstOrNull()
}
