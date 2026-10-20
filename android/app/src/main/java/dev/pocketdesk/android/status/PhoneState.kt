package dev.pocketdesk.android.status

import android.content.Context
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.StatFs
import android.telephony.TelephonyManager
import android.util.Log

/**
 * The phone's state now (L9), as [PhoneStateRule] says it: each part read
 * on its own, and left out when Android will not say.
 */
internal fun readPhoneState(context: Context): PhoneStateRule.State {
    val storage = runCatching {
        val stat = StatFs(context.filesDir.path)
        PhoneStateRule.storage(stat.availableBytes, stat.totalBytes)
    }.getOrNull()
    val (network, signal) = runCatching { network(context) }
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

/** The default network's kind and bars: never its name. */
private fun network(context: Context): Pair<String?, Int?> {
    val cm = context.getSystemService(ConnectivityManager::class.java) ?: return null to null
    val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) }
        ?: return PhoneStateRule.NONE to null
    return when {
        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> {
            val bars = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                PhoneStateRule.wifiBars(caps.signalStrength)
            } else {
                null
            }
            PhoneStateRule.WIFI to bars
        }
        caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> {
            val bars = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                // Needs no permission; a phone that refuses says no bars.
                runCatching {
                    context.getSystemService(TelephonyManager::class.java)?.signalStrength?.level
                }.getOrNull()?.let(PhoneStateRule::mobileBars)
            } else {
                null
            }
            PhoneStateRule.MOBILE to bars
        }
        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> PhoneStateRule.ETHERNET to null
        // A VPN alone, Bluetooth or USB tethering: none of the three.
        else -> null to null
    }
}
