package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.AlsoToPrefs
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.status.AllowedRule
import dev.pocketdesk.android.status.readSwitches
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * What a computer allows this phone (M2a): each of L7's features with
 * this phone's switch and Android's grant beside the computer's permission
 * for this phone and its own Settings. Asked once as it opens; Retry asks
 * again.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AllowedSheet(address: String, controlPort: Int, computer: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var rows by remember { mutableStateOf<List<AllowedRule.Row>?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var asks by remember { mutableIntStateOf(0) }

    LaunchedEffect(address, controlPort, asks) {
        failure = null
        val (switches, reach) = withContext(Dispatchers.IO) {
            runCatching { readSwitches(context) }.getOrDefault(emptyMap()) to reachOf(context, address)
        }
        withContext(Dispatchers.IO) { RustCore.allowed(address, controlPort) }
            .mapCatching { parseAllowed(it) }
            .onSuccess { rows = AllowedRule.rows(it, switches, reach) }
            .onFailure {
                android.util.Log.i("AllowedSheet", "No answer: ${it.message}")
                failure = it.message.orEmpty()
            }
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.allowed_title, computer), style = MaterialTheme.typography.titleMedium)
            Text(stringResource(R.string.allowed_hint), style = MaterialTheme.typography.bodySmall)
            val shown = rows
            when {
                failure != null -> {
                    Text(stringResource(R.string.allowed_failed))
                    TextButton(onClick = { asks++ }) { Text(stringResource(R.string.retry)) }
                }
                shown == null -> Text(stringResource(R.string.allowed_asking))
                shown.all { it.computer == AllowedRule.Computer.UNKNOWN } ->
                    Text(stringResource(R.string.allowed_not_paired))
                else -> shown.forEach { AllowedRowView(it) }
            }
        }
    }
}

/**
 * What the phone's background features reach on the computer at [address]:
 * its own computer (the same one at another address too), one picked under
 * "Other computers of mine", or another (M2 review).
 */
private fun reachOf(ctx: android.content.Context, address: String): AllowedRule.Reach {
    val here = HostStore.pairedDesktopId(ctx, address)
    val own = PhoneLink.target(ctx)
    val ownId = own?.let { HostStore.pairedDesktopId(ctx, it.address) }
    return when {
        own?.address == address || (here != null && here == ownId) -> AllowedRule.Reach.OWN
        here != null && here in AlsoToPrefs.picked(ctx) -> AllowedRule.Reach.COPIES
        else -> AllowedRule.Reach.OTHER
    }
}

/** The computer's answer: each feature's two booleans; anything else left out. */
private fun parseAllowed(json: String): Map<String, AllowedRule.Side> {
    val allowed = JSONObject(json).optJSONObject("allowed") ?: return emptyMap()
    return allowed.keys().asSequence().mapNotNull { key ->
        val side = allowed.optJSONObject(key) ?: return@mapNotNull null
        if (!side.has("permission") || !side.has("setting")) return@mapNotNull null
        key to AllowedRule.Side(side.optBoolean("permission"), side.optBoolean("setting"))
    }.toMap()
}

@Composable
private fun AllowedRowView(row: AllowedRule.Row) {
    val flagged = row.heldBack
    Column(
        modifier = Modifier.semantics(mergeDescendants = true) {},
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(stringResource(featureName(row.feature)), style = MaterialTheme.typography.bodyMedium)
        val phone = when (row.phone) {
            AllowedRule.Phone.ON -> R.string.allowed_phone_on
            AllowedRule.Phone.OFF -> R.string.allowed_phone_off
            AllowedRule.Phone.ANDROID_REFUSES -> R.string.allowed_phone_android
            AllowedRule.Phone.OWN_COMPUTER_ONLY -> R.string.allowed_phone_own_only
        }
        val computer = when (row.computer) {
            AllowedRule.Computer.ALLOWED -> R.string.allowed_computer_yes
            AllowedRule.Computer.PERMISSION_OFF -> R.string.allowed_computer_permission
            AllowedRule.Computer.SETTING_OFF -> R.string.allowed_computer_setting
            AllowedRule.Computer.UNKNOWN -> R.string.allowed_computer_unknown
        }
        val color = if (flagged) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant
        Text(stringResource(phone), style = MaterialTheme.typography.bodySmall, color = color)
        Text(stringResource(computer), style = MaterialTheme.typography.bodySmall, color = color)
    }
}

/** Each feature by the phone's own Settings switch for it. */
private fun featureName(feature: String): Int = when (feature) {
    "notifications" -> R.string.phone_notifications_share
    "answers" -> R.string.phone_notifications_answer
    "alarms" -> R.string.phone_notifications_alarms
    "texts" -> R.string.texts_show
    "contacts" -> R.string.texts_contacts
    "clipboard" -> R.string.settings_clipboard_background
    "copies" -> R.string.settings_copies
    "files" -> R.string.settings_files_background
    "images" -> R.string.settings_received_images_clipboard
    "folders" -> R.string.settings_phone_folders
    "photos" -> R.string.recent_photos_on
    "screenshots" -> R.string.screenshots_on
    "camera_photos" -> R.string.camera_uploads_on
    "camera" -> R.string.settings_camera_asks
    "links" -> R.string.settings_links_from_computer
    "ring" -> R.string.settings_ring_from_computer
    "media" -> R.string.settings_phone_media
    "media_background" -> R.string.settings_phone_media_background
    "computer_media" -> R.string.settings_computer_media_controls
    "mute" -> R.string.settings_let_computer_mute
    "control" -> R.string.settings_control_from_computer
    "typing" -> R.string.settings_typing_from_computer
    "dial" -> R.string.settings_calls_from_computer
    "commands" -> R.string.settings_commands_from_computer
    "tasks" -> R.string.settings_others_run_tasks
    "phone_actions" -> R.string.settings_phone_actions
    "apps" -> R.string.settings_phone_apps
    // Only SwitchReport.FEATURES come here (Q3 review: a new one had read
    // as Commands).
    else -> error("no label for the feature $feature")
}
