package dev.pocketdesk.android.ui

import android.Manifest
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
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.TextPrefs

/**
 * Settings: texts on the computer (N5b). "Show texts" asks for RECEIVE_SMS,
 * "Reply from my computer" for SEND_SMS, each only when turned on. Replies
 * also need notification access, whose service keeps the connection they
 * arrive on; the row says so when it is missing.
 */
@Composable
fun TextsSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val show = version.let { TextPrefs.show(context) }
    val reply = version.let { TextPrefs.reply(context) }
    val canReceive = version.let { TextPrefs.granted(context, Manifest.permission.RECEIVE_SMS) }
    val canSend = version.let { TextPrefs.granted(context, Manifest.permission.SEND_SMS) }
    val names = version.let { TextPrefs.names(context) }
    val canReadContacts = version.let { TextPrefs.granted(context, Manifest.permission.READ_CONTACTS) }
    val listening = version.let { PhoneNotificationPrefs.accessGranted(context) }

    val askReceive =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setShow(context, granted)
            version++
        }
    val askSend =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setReply(context, granted)
            version++
        }

    val askContacts =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setNames(context, granted)
            version++
        }

    fun setNames(on: Boolean) {
        if (on && !canReadContacts) {
            askContacts.launch(Manifest.permission.READ_CONTACTS)
        } else {
            TextPrefs.setNames(context, on)
            version++
        }
    }

    fun setShow(on: Boolean) {
        if (on && !canReceive) {
            askReceive.launch(Manifest.permission.RECEIVE_SMS)
        } else {
            TextPrefs.setShow(context, on)
            version++
        }
    }

    fun setReply(on: Boolean) {
        if (on && !canSend) {
            askSend.launch(Manifest.permission.SEND_SMS)
        } else {
            TextPrefs.setReply(context, on)
            // Turned off: requests waiting from before are never sent later.
            if (!on) dev.pocketdesk.android.bridge.RustCore.takeSmsRequests()
            version++
        }
    }

    Text(
        stringResource(R.string.settings_texts),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 2.dp),
    )
    ListItem(
        headlineContent = { Text(stringResource(R.string.texts_show), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            Text(stringResource(R.string.texts_show_summary), style = MaterialTheme.typography.bodySmall)
        },
        leadingContent = { LlIcon(LlIcons.Notifications, null) },
        trailingContent = { Switch(checked = show && canReceive, onCheckedChange = ::setShow) },
        modifier = Modifier.clickable { setShow(!(show && canReceive)) },
    )
    ListItem(
        headlineContent = { Text(stringResource(R.string.texts_reply), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            val needsAccess = reply && canSend && !listening
            Text(
                stringResource(if (needsAccess) R.string.texts_reply_needs_access else R.string.texts_reply_summary),
                style = MaterialTheme.typography.bodySmall,
                color = if (needsAccess) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        },
        trailingContent = { Switch(checked = reply && canSend, onCheckedChange = ::setReply) },
        modifier =
            Modifier.clickable {
                if (reply && canSend && !listening) {
                    runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
                } else {
                    setReply(!(reply && canSend))
                }
            },
    )
    ListItem(
        headlineContent = { Text(stringResource(R.string.texts_names), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            Text(stringResource(R.string.texts_names_summary), style = MaterialTheme.typography.bodySmall)
        },
        trailingContent = { Switch(checked = names && canReadContacts, onCheckedChange = ::setNames) },
        modifier = Modifier.clickable { setNames(!(names && canReadContacts)) },
    )
}
