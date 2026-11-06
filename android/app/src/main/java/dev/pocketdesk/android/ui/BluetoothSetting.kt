package dev.pocketdesk.android.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bluetooth.BluetoothRoad
import dev.pocketdesk.android.bluetooth.BluetoothRoadRule

/**
 * "Keep working over Bluetooth when Wi-Fi drops" (the Bluetooth fallback),
 * on by default. Without Android's nearby devices permission it does
 * nothing, and says so; turning it on asks for the permission (only on
 * Android 12 and later: before, [BluetoothRoad.hasPermission] is true).
 */
@android.annotation.SuppressLint("InlinedApi")
@Composable
fun BluetoothFallbackSetting() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.bluetoothFallback(context) }
    val permitted = (version + resumes).let { BluetoothRoad.hasPermission(context) }
    val says = BluetoothRoadRule.switchSays(on, permitted)

    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        if (ok) Prefs.setBluetoothFallback(context, true)
        version++
    }

    SettingSwitch(
        stringResource(R.string.settings_bluetooth_fallback),
        stringResource(
            if (says == BluetoothRoadRule.SwitchSays.NEEDS_PERMISSION) {
                R.string.settings_bluetooth_fallback_needs_permission
            } else {
                R.string.settings_bluetooth_fallback_summary
            },
        ),
        says == BluetoothRoadRule.SwitchSays.WORKS,
        { value ->
            if (value && !permitted) {
                ask.launch(android.Manifest.permission.BLUETOOTH_CONNECT)
            } else {
                Prefs.setBluetoothFallback(context, value)
                // A live road ends by itself within a second once the switch is off.
                version++
            }
        },
        warning = says == BluetoothRoadRule.SwitchSays.NEEDS_PERMISSION,
    )
}
