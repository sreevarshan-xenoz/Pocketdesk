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
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.ControlLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

internal const val BATTERY = "kdeconnect.battery"
private const val CHECK_MS = 15_000L

/**
 * Tells the computer of this session the phone's battery (T8b), as
 * [BatteryRule] says, while "Share battery with the computer" is on. It
 * reads Android's last battery broadcast every 15 s while the app is
 * started, and sends only on the session's own control connection
 * ([ControlLinkEffect][dev.pocketdesk.android.ui.ControlLinkEffect] keeps
 * it up): never a connection of its own, so a computer that is away is not
 * dialed every 15 s. What was not sent goes at the next check the link is up.
 */
@Composable
fun BatteryEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        var last: BatteryRule.Report? = null
        var lastMs = 0L
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (isActive) {
                val linked = withContext(Dispatchers.IO) {
                    !ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)
                }
                val now = if (linked && Prefs.shareBattery(context)) readBattery(context) else null
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
}

/** The battery now, from the sticky broadcast (no receiver is kept). */
internal fun readBattery(context: Context): BatteryRule.Report? {
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
