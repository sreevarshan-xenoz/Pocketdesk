package dev.pocketdesk.android.status

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioManager
import android.os.BatteryManager
import android.os.Build
import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.ControlLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal const val BATTERY = "kdeconnect.battery"
private const val CHECK_MS = 15_000L

/**
 * Tells the computer of this session the phone's battery (T8b), as
 * [BatteryRule] says, while "Share the phone's state with the computer" is on. It
 * reads Android's last battery broadcast every 15 s while the app is
 * started, and sends only on the session's own control connection
 * ([ControlLinkEffect][dev.pocketdesk.android.ui.ControlLinkEffect] keeps
 * it up): never a connection of its own, so a computer that is away is not
 * dialed every 15 s. What was not sent goes at the next check the link is up.
 * A ringer turned is checked at once, not at the next check (AP3).
 */
@Composable
fun BatteryEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        var last: BatteryRule.Report? = null
        var lastMs = 0L
        // L7: which features are on, told with the battery's ticks.
        val switches = SwitchSender()
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            val wake = TickWake()
            launch { pokeOnRingerChange(context, wake) }
            while (isActive) {
                val linked = withContext(Dispatchers.IO) {
                    !ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)
                }
                // Read off the main thread (L9 review: binder calls and a
                // statfs); storage and bars only for a report that goes.
                val light = if (linked && Prefs.shareBattery(context)) {
                    withContext(Dispatchers.IO) { readBattery(context, full = false) }
                } else {
                    null
                }
                val nowMs = SystemClock.elapsedRealtime()
                if (light != null && BatteryRule.shouldSend(last, lastMs, light, nowMs)) {
                    val now = withContext(Dispatchers.IO) { readBattery(context) } ?: light
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
                if (linked && Prefs.shareBattery(context)) {
                    withContext(Dispatchers.IO) {
                        // A read that throws is skipped, never the app (L7 review).
                        val said = runCatching { readSwitches(context) }.getOrNull()
                        if (said != null) switches.tick(said, nowMs) { body ->
                            RustCore.sendPhonePacket(address, controlPort, SwitchReport.SWITCHES, body)
                        }
                    }
                } else {
                    switches.forget()
                }
                wake.await(CHECK_MS)
            }
        }
    }
}

/** Pokes [wake] each time the ringer changes, until cancelled. */
private suspend fun pokeOnRingerChange(context: Context, wake: TickWake) {
    val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = wake.poke()
    }
    // A system broadcast, so it reaches a receiver not exported.
    ContextCompat.registerReceiver(
        context,
        receiver,
        IntentFilter(AudioManager.RINGER_MODE_CHANGED_ACTION),
        ContextCompat.RECEIVER_NOT_EXPORTED,
    )
    try {
        awaitCancellation()
    } finally {
        context.unregisterReceiver(receiver)
    }
}

/**
 * The battery now, from the sticky broadcast (no receiver is kept), with the
 * phone's state (L9): all of it when [full], else only what decides a send
 * (the network's kind and the ringer), which costs less.
 */
internal fun readBattery(context: Context, full: Boolean = true): BatteryRule.Report? {
    val intent: Intent? = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
    val sticky = intent?.let {
        val low = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P &&
            it.getBooleanExtra(BatteryManager.EXTRA_BATTERY_LOW, false)
        BatteryRule.report(
            level = it.getIntExtra(BatteryManager.EXTRA_LEVEL, -1),
            scale = it.getIntExtra(BatteryManager.EXTRA_SCALE, -1),
            plugged = it.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0),
            low = low,
        )
    }
    // Asked of BatteryManager too: the broadcast was seen stale in the
    // background on ColorOS (found checking L3 on the OPPO).
    val manager = context.getSystemService(BatteryManager::class.java)
    val report = BatteryRule.fresh(
        sticky,
        manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: Int.MIN_VALUE,
        manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_STATUS) ?: Int.MIN_VALUE,
    )
    return report?.copy(state = readPhoneState(context, full))
}
