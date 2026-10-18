package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings: whether this phone's notifications appear on the computer (N5a),
 * off by default, and which apps share theirs: each app that has notified is
 * listed, off until switched on, so no app's notification reaches the
 * computer before the user chose it. Read again on resume, so a grant made in
 * Android's settings shows when the user comes back.
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
    val shared = version.let { PhoneNotificationPrefs.shared(context) }
    val seen = version.let { PhoneNotificationPrefs.seen(context) }

    fun setEnabled(on: Boolean) {
        PhoneNotificationPrefs.setEnabled(context, on)
        if (on && !PhoneNotificationPrefs.accessGranted(context)) {
            runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
        }
        version++
    }

    val answer = version.let { Prefs.answerNotifications(context) }

    SettingSwitch(
        stringResource(R.string.phone_notifications_share),
        stringResource(if (enabled && !granted) R.string.phone_notifications_no_access else R.string.phone_notifications_share_summary),
        enabled,
        ::setEnabled,
        warning = enabled && !granted,
    )
    if (enabled && !granted) AllowNotificationAccess()
    // T10c: the computer may press buttons and reply, as this phone's owner.
    SettingSwitch(
        stringResource(R.string.phone_notifications_answer),
        stringResource(R.string.phone_notifications_answer_summary),
        answer,
        {
            Prefs.setAnswerNotifications(context, it)
            version++
        },
        enabled = enabled,
    )
    if (!enabled) return
    // D2: what Do Not Disturb hides on the phone stays off the computer.
    val holdBack = version.let { dev.pocketdesk.android.notify.QuietPrefs.holdBack(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_hold_back),
        stringResource(R.string.phone_notifications_hold_back_summary),
        holdBack,
        {
            dev.pocketdesk.android.notify.QuietPrefs.setHoldBack(context, it)
            version++
        },
    )
    // C3: how long a notification waits for a computer out of reach.
    val wait = version.let { PhoneNotificationPrefs.waitMinutes(context) }
    SettingRow(stringResource(R.string.phone_notifications_wait), stringResource(R.string.phone_notifications_wait_summary))
    Column(Modifier.selectableGroup()) {
        PhoneNotificationPrefs.WAIT_CHOICES.forEach { minutes ->
            val label = when (minutes) {
                60 -> R.string.phone_notifications_wait_60
                10 -> R.string.phone_notifications_wait_10
                else -> R.string.phone_notifications_wait_0
            }
            ListItem(
                headlineContent = { Text(stringResource(label), style = MaterialTheme.typography.bodyLarge) },
                leadingContent = { RadioButton(selected = wait == minutes, onClick = null) },
                modifier = Modifier
                    .selectable(selected = wait == minutes, role = Role.RadioButton) {
                        PhoneNotificationPrefs.setWaitMinutes(context, minutes)
                        version++
                    }
                    .padding(start = 24.dp),
            )
        }
    }
    if (seen.isEmpty()) {
        Text(
            stringResource(R.string.phone_notifications_apps_empty),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
    seen.forEach { (pkg, label) ->
        SettingSwitch(
            label,
            pkg,
            pkg in shared,
            {
                PhoneNotificationPrefs.setShared(context, pkg, it)
                version++
            },
            indent = true,
        )
    }
}
