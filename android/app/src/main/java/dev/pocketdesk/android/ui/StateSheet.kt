package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** How often the sheet asks again while it is open. */
private const val REFRESH_MS = 3_000L

/**
 * How the computer is doing (F2): its CPU, memory and disks as bars, its
 * battery and how long it has been up, asked again every few seconds while
 * the sheet is open.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StateSheet(address: String, controlPort: Int, computer: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var state by remember { mutableStateOf<ComputerState.State?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(address, controlPort) {
        while (isActive) {
            withContext(Dispatchers.IO) { RustCore.systemState(address, controlPort) }
                .mapCatching { ComputerState.parse(it) }
                .onSuccess {
                    state = it
                    failure = null
                }
                .onFailure { if (state == null) failure = context.plainErrorText(it.message) }
            delay(REFRESH_MS)
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
            Text(stringResource(R.string.state_title, computer), style = MaterialTheme.typography.titleMedium)
            val shown = state
            val of = stringResource(R.string.state_of)
            when {
                failure != null -> Text(stringResource(R.string.state_failed, failure.orEmpty()))
                shown == null -> Text(stringResource(R.string.state_loading))
                shown.off -> Text(stringResource(R.string.state_not_shared))
                else -> {
                    shown.cpu?.let { cpu ->
                        Bar(stringResource(R.string.state_cpu), "%.0f %%".format(cpu), cpu / 100f)
                    }
                    shown.memory?.let { Bar(stringResource(R.string.state_memory), it.words(of), it.fraction) }
                    shown.disks.forEach { disk ->
                        Bar(stringResource(R.string.state_disk, disk.name), disk.words(of), disk.fraction)
                    }
                    shown.batteryCharge?.let { charge ->
                        Bar(
                            stringResource(R.string.state_battery),
                            if (shown.batteryCharging) {
                                stringResource(R.string.state_battery_charging, charge)
                            } else {
                                "$charge %"
                            },
                            charge / 100f,
                        )
                    }
                    shown.uptimeSecs?.let { secs ->
                        Row(horizontalArrangement = Arrangement.SpaceBetween, modifier = Modifier.fillMaxWidth()) {
                            Text(stringResource(R.string.state_uptime), style = MaterialTheme.typography.bodyMedium)
                            Text(
                                ComputerState.uptime(
                                    secs,
                                    stringResource(R.string.state_unit_day),
                                    stringResource(R.string.state_unit_hour),
                                    stringResource(R.string.state_unit_minute),
                                ),
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                    }
                }
            }
        }
    }
}

/** A labelled bar with its words on the right. */
@Composable
private fun Bar(label: String, words: String, fraction: Float) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.SpaceBetween, modifier = Modifier.fillMaxWidth()) {
            Text(label, style = MaterialTheme.typography.bodyMedium)
            Text(words, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        LinearProgressIndicator(progress = { fraction.coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
    }
}
