package dev.pocketdesk.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.doctor.ConnectionChecks
import dev.pocketdesk.android.doctor.ConnectionProbe
import dev.pocketdesk.android.doctor.DoctorReport
import dev.pocketdesk.android.doctor.Finding
import dev.pocketdesk.android.doctor.FixAction
import dev.pocketdesk.android.doctor.Status
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The Connection Doctor (T1): checks why this phone cannot reach a saved
 * computer, and says what to do. It runs when opened and on "Check again",
 * never in the background.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectionDoctorSheet(
    address: String,
    streamPort: Int,
    controlPort: Int,
    canWake: Boolean,
    onWake: () -> Unit,
    onPairAgain: () -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    var findings by remember { mutableStateOf<List<Finding>?>(null) }
    var run by remember { mutableIntStateOf(0) }
    LaunchedEffect(address, run) {
        findings = null
        findings = withContext(Dispatchers.IO) {
            val state = ConnectionProbe.phoneState(context)
            val probes = ConnectionProbe.probe(context, address, controlPort, streamPort, state.vpn)
            ConnectionChecks.run(state, address, probes, canWake)
        }
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(modifier = Modifier.padding(horizontal = 16.dp)) {
            Text(stringResource(R.string.doctor_title), style = MaterialTheme.typography.titleLarge)
            Text(
                address,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
            val shown = findings
            if (shown == null) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 24.dp),
                    horizontalArrangement = Arrangement.Center,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    CircularProgressIndicator()
                    Text(
                        stringResource(R.string.doctor_running),
                        modifier = Modifier.padding(start = 16.dp),
                    )
                }
            } else {
                LazyColumn(modifier = Modifier.weight(1f, fill = false)) {
                    items(shown, key = { it.code }) { finding ->
                        FindingRow(finding) { act(context, finding.action, onWake, onPairAgain) }
                    }
                }
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 12.dp),
                    horizontalArrangement = Arrangement.End,
                ) {
                    TextButton(onClick = { copyReport(context, shown) }) {
                        Text(stringResource(R.string.doctor_copy_report))
                    }
                    TextButton(onClick = { run++ }) {
                        Text(stringResource(R.string.doctor_run_again))
                    }
                }
            }
        }
    }
}

@Composable
private fun FindingRow(finding: Finding, onAction: () -> Unit) {
    val (glyph, tint) = when (finding.status) {
        Status.GOOD -> LlIcons.Check to MaterialTheme.colorScheme.primary
        Status.WARN -> LlIcons.Warning to MaterialTheme.colorScheme.tertiary
        Status.BAD -> LlIcons.Warning to MaterialTheme.colorScheme.error
        Status.INFO -> LlIcons.Info to MaterialTheme.colorScheme.onSurfaceVariant
    }
    ListItem(
        leadingContent = { LlIcon(glyph, null, tint = tint) },
        headlineContent = { Text(stringResource(finding.message)) },
        supportingContent = finding.fix?.let { fix ->
            {
                Column {
                    Text(stringResource(fix))
                    if (finding.action != FixAction.NONE) {
                        TextButton(onClick = onAction) { Text(stringResource(actionLabel(finding.action))) }
                    }
                }
            }
        },
    )
}

private fun actionLabel(action: FixAction): Int = when (action) {
    FixAction.WAKE -> R.string.doctor_action_wake
    FixAction.PAIR_AGAIN -> R.string.doctor_action_pair
    FixAction.ASK_LOCAL_NETWORK -> R.string.doctor_action_allow
    else -> R.string.doctor_action_settings
}

private fun act(context: Context, action: FixAction, onWake: () -> Unit, onPairAgain: () -> Unit) {
    val appUri = Uri.fromParts("package", context.packageName, null)
    val intent = when (action) {
        FixAction.NONE -> return
        FixAction.WAKE -> return onWake()
        FixAction.PAIR_AGAIN -> return onPairAgain()
        FixAction.ASK_LOCAL_NETWORK, FixAction.OPEN_APP_SETTINGS ->
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, appUri)
        FixAction.OPEN_WIFI_SETTINGS -> Intent(Settings.ACTION_WIFI_SETTINGS)
        FixAction.OPEN_BATTERY_SETTINGS -> Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
        FixAction.OPEN_DATA_SAVER -> Intent(Settings.ACTION_IGNORE_BACKGROUND_DATA_RESTRICTIONS_SETTINGS, appUri)
        FixAction.OPEN_NOTIFICATION_ACCESS -> Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
    }
    runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

private fun copyReport(context: Context, findings: List<Finding>) {
    val version = runCatching {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName
    }.getOrNull() ?: "?"
    val report = DoctorReport.text(findings, Build.VERSION.RELEASE, Build.MANUFACTURER, version)
    context.getSystemService(ClipboardManager::class.java)
        .setPrimaryClip(ClipData.newPlainText("Pocketdesk", report))
    Toast.makeText(context, R.string.doctor_copied, Toast.LENGTH_SHORT).show()
}
