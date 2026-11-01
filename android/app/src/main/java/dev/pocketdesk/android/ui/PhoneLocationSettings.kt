package dev.pocketdesk.android.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.R
import dev.pocketdesk.android.location.PhoneLocationPrefs

/**
 * Settings "Give my computer my location" (GeoClue): off by default; on, it
 * asks for the coarse location permission, which gives the city (about 1 km).
 * Under it, "Exact location" (off) asks for the fine one, and, where Android
 * has it, a link to allow location all the time, which is only for an ask
 * while Pocketdesk is not on screen. The computer asks on the link the
 * notification listener keeps, so without notification access it says so.
 */
@Composable
fun PhoneLocationSetting() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val state = version + resumes
    val on = state.let { PhoneLocationPrefs.on(context) }
    val coarse = state.let { PhoneLocationPrefs.coarseGranted(context) }
    val fine = state.let { PhoneLocationPrefs.fineGranted(context) }
    val exact = state.let { PhoneLocationPrefs.exact(context) }
    val background = state.let { PhoneLocationPrefs.backgroundGranted(context) }
    val access = state.let { dev.pocketdesk.android.notify.PhoneNotificationPrefs.accessGranted(context) }

    val askCoarse =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            PhoneLocationPrefs.setOn(context, ok)
            version++
        }
    val askFine =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            PhoneLocationPrefs.setExact(context, ok)
            version++
        }
    val summary =
        when {
            on && !coarse -> R.string.settings_location_needs_permission
            on && !access -> R.string.settings_location_needs_access
            else -> R.string.settings_location_summary
        }
    SettingSwitch(
        stringResource(R.string.settings_location),
        stringResource(summary),
        on && coarse,
        { want ->
            if (want && !coarse) {
                askCoarse.launch(PhoneLocationPrefs.COARSE)
            } else {
                PhoneLocationPrefs.setOn(context, want)
                version++
            }
        },
        warning = on && (!coarse || !access),
    )
    if (on && coarse && !access) AllowNotificationAccess()
    if (on && coarse) {
        SettingSwitch(
            stringResource(R.string.settings_location_exact),
            stringResource(R.string.settings_location_exact_summary),
            exact && fine,
            { want ->
                if (want && !fine) {
                    askFine.launch(PhoneLocationPrefs.FINE)
                } else {
                    PhoneLocationPrefs.setExact(context, want)
                    version++
                }
            },
            indent = true,
        )
        // Android 10 and later show no location to an app that is not on
        // screen unless its owner chose "Allow all the time" for it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && !background) {
            SettingLink(
                stringResource(R.string.settings_location_background),
                stringResource(R.string.settings_location_background_summary),
            ) {
                runCatching {
                    context.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                }
            }
        }
    }
}
