package dev.pocketdesk.android.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
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
    Text(
        stringResource(R.string.settings_screenshots),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 2.dp),
    )
    ListItem(
        headlineContent = { Text(stringResource(R.string.screenshots_on), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            val needsAccess = active && !listening
            Text(
                stringResource(
                    when {
                        needsAccess -> R.string.screenshots_needs_access
                        on && !granted -> R.string.screenshots_needs_photos
                        else -> R.string.screenshots_on_summary
                    },
                ),
                style = MaterialTheme.typography.bodySmall,
                color = if (needsAccess || (on && !granted)) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        },
        trailingContent = { Switch(checked = active, onCheckedChange = ::setOn) },
        modifier =
            Modifier.clickable {
                if (active && !listening) {
                    runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
                } else {
                    setOn(!active)
                }
            },
    )
    if (active) {
        ListItem(
            headlineContent = { Text(stringResource(R.string.screenshots_clipboard), style = MaterialTheme.typography.bodyLarge) },
            trailingContent = {
                Switch(checked = clipboard, onCheckedChange = {
                    ScreenshotPrefs.setClipboard(context, it)
                    version++
                })
            },
            modifier = Modifier.padding(start = 24.dp),
        )
        ListItem(
            headlineContent = { Text(stringResource(R.string.screenshots_save), style = MaterialTheme.typography.bodyLarge) },
            trailingContent = {
                Switch(checked = save, onCheckedChange = {
                    ScreenshotPrefs.setSave(context, it)
                    version++
                })
            },
            modifier = Modifier.padding(start = 24.dp),
        )
    }
}
