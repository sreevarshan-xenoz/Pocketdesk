package dev.pocketdesk.android.status

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

private const val BATTERY = "kdeconnect.battery"
private const val CHECK_MS = 15_000L

/**
 * Tells the computer of this session the phone's battery (T8b), as
 * [BatteryRule] says, while "Share battery with the computer" is on. It
 * reads Android's last battery broadcast every 15 s; a send that fails is
 * tried again at the next check.
 */
@Composable
fun BatteryEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    LaunchedEffect(address, controlPort) {
        var last: BatteryRule.Report? = null
        var lastMs = 0L
        while (isActive) {
            val now = if (Prefs.shareBattery(context)) read(context) else null
            val nowMs = SystemClock.elapsedRealtime()
            if (now != null && BatteryRule.shouldSend(last, lastMs, now, nowMs)) {
                val sent = withContext(Dispatchers.IO) {
                    RustCore.sendPhonePacket(address, controlPort, BATTERY, BatteryRule.body(now))
                }
                if (sent.isSuccess) {
                    last = now
                    lastMs = nowMs
                } else {
                    Log.d("BatteryEffect", "Battery not sent: ${sent.exceptionOrNull()?.message}")
                }
            }
            delay(CHECK_MS)
        }
    }
}

/** The battery now, from the sticky broadcast (no receiver is kept). */
private fun read(context: Context): BatteryRule.Report? {
    val intent: Intent = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        ?: return null
    val low = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P &&
        intent.getBooleanExtra(BatteryManager.EXTRA_BATTERY_LOW, false)
    return BatteryRule.report(
        level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1),
        scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, -1),
        plugged = intent.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0),
        low = low,
    )
}
