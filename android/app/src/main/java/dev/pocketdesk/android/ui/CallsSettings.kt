package dev.pocketdesk.android.ui

import android.Manifest
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
import dev.pocketdesk.android.notify.CallWatch
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings: calls on the computer (F5). "Ringing calls on the computer"
 * asks for the phone's state and call log (the number), "Let the computer
 * decline calls" for ending calls, each only when turned on. The listener
 * watches the calls and keeps the link the answers come on, so the row
 * says when notification access is missing.
 */
@Composable
fun CallsSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.callsOn(context) }
    val decline = (version + resumes).let { Prefs.callsDecline(context) }
    val canWatch = (version + resumes).let { CallWatch.permissions.all { p -> CallWatch.granted(context, p) } }
    val canEnd = (version + resumes).let { CallWatch.granted(context, Manifest.permission.ANSWER_PHONE_CALLS) }
    val listening = (version + resumes).let { PhoneNotificationPrefs.accessGranted(context) }

    val askWatch =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
            Prefs.setCallsOn(context, results.values.all { it })
            version++
        }
    val askEnd =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            Prefs.setCallsDecline(context, granted)
            version++
        }

    val active = on && canWatch
    val needsAccess = active && !listening
    SettingSwitch(
        stringResource(R.string.settings_calls),
        stringResource(if (needsAccess) R.string.settings_calls_needs_access else R.string.settings_calls_summary),
        active,
        { value ->
            if (value && !canWatch) {
                askWatch.launch(CallWatch.permissions)
            } else {
                Prefs.setCallsOn(context, value)
                version++
            }
        },
        warning = needsAccess,
    )
    if (needsAccess) AllowNotificationAccess()
    if (active) {
        SettingSwitch(
            stringResource(R.string.settings_calls_decline),
            stringResource(R.string.settings_calls_decline_summary),
            decline && canEnd,
            { value ->
                if (value && !canEnd) {
                    askEnd.launch(Manifest.permission.ANSWER_PHONE_CALLS)
                } else {
                    Prefs.setCallsDecline(context, value)
                    version++
                }
            },
        )
    }
}
