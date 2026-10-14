package dev.pocketdesk.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.R
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings: whether this phone's notifications appear on the computer (N5a),
 * off by default, and which apps are left out. Read again on resume, so a
 * grant made in Android's settings shows when the user comes back.
 */
@Composable
fun PhoneNotificationsSection() {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var version by remember { mutableIntStateOf(0) }
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) version++ }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    // Read under `version`, so each change and each resume recomposes.
    val enabled = version.let { PhoneNotificationPrefs.enabled(context) }
    val granted = version.let { PhoneNotificationPrefs.accessGranted(context) }
    val excluded = version.let { PhoneNotificationPrefs.excluded(context) }
    val seen = version.let { PhoneNotificationPrefs.seen(context) }

    fun setEnabled(on: Boolean) {
        PhoneNotificationPrefs.setEnabled(context, on)
        if (on && !PhoneNotificationPrefs.accessGranted(context)) {
            runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
        }
        version++
    }

    Text(
        stringResource(R.string.settings_phone_notifications),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 2.dp),
    )
    ListItem(
        headlineContent = { Text(stringResource(R.string.phone_notifications_share), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            val summary =
                if (enabled && !granted) R.string.phone_notifications_no_access else R.string.phone_notifications_share_summary
            Text(
                stringResource(summary),
                style = MaterialTheme.typography.bodySmall,
                color = if (enabled && !granted) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        },
        leadingContent = { LlIcon(LlIcons.Notifications, null) },
        trailingContent = { Switch(checked = enabled, onCheckedChange = ::setEnabled) },
        modifier =
            Modifier.clickable {
                if (enabled && !granted) {
                    runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
                } else {
                    setEnabled(!enabled)
                }
            },
    )
    if (!enabled) return
    if (seen.isEmpty()) {
        Text(
            stringResource(R.string.phone_notifications_apps_empty),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
    seen.forEach { (pkg, label) ->
        val shared = pkg !in excluded
        ListItem(
            headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
            supportingContent = { Text(pkg, style = MaterialTheme.typography.bodySmall) },
            trailingContent = {
                Switch(
                    checked = shared,
                    onCheckedChange = {
                        PhoneNotificationPrefs.setExcluded(context, pkg, !it)
                        version++
                    },
                )
            },
            modifier =
                Modifier.clickable {
                    PhoneNotificationPrefs.setExcluded(context, pkg, shared)
                    version++
                },
        )
    }
}
