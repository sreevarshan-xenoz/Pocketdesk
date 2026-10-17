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
import dev.pocketdesk.android.R
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings "New screenshots to the computer" (T10h): off by default; on, it
 * asks for photo access, and two switches say where a screenshot goes. The
 * notification listener does the work, so the row says when its access is
 * missing.
 */
@Composable
fun ScreenshotsSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val on = version.let { ScreenshotPrefs.on(context) }
    val granted = version.let { ScreenshotPrefs.granted(context) }
    val clipboard = version.let { ScreenshotPrefs.clipboard(context) }
    val save = version.let { ScreenshotPrefs.save(context) }
    val listening = version.let { PhoneNotificationPrefs.accessGranted(context) }

    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            ScreenshotPrefs.setOn(context, ok)
            version++
        }

    fun setOn(value: Boolean) {
        if (value && !granted) {
            ask.launch(ScreenshotPrefs.permission)
        } else {
            ScreenshotPrefs.setOn(context, value)
            version++
        }
    }

    val active = on && granted
    val needsAccess = active && !listening
    SettingSwitch(
        stringResource(R.string.screenshots_on),
        stringResource(
            when {
                needsAccess -> R.string.screenshots_needs_access
                on && !granted -> R.string.screenshots_needs_photos
                else -> R.string.screenshots_on_summary
            },
        ),
        active,
        ::setOn,
        warning = needsAccess || (on && !granted),
    )
    if (needsAccess) AllowNotificationAccess()
    if (active) {
        SettingSwitch(
            stringResource(R.string.screenshots_clipboard),
            null,
            clipboard,
            {
                ScreenshotPrefs.setClipboard(context, it)
                version++
            },
            indent = true,
        )
        SettingSwitch(
            stringResource(R.string.screenshots_save),
            null,
            save,
            {
                ScreenshotPrefs.setSave(context, it)
                version++
            },
            indent = true,
        )
    }
}
