package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.status.DevicesThereRule
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Remote revoke: the devices paired with a computer, each with Remove, for
 * a lost or stolen one. The computer answers only a phone paired with its
 * owner's own code, says who removed what, and keeps no undo: the device
 * pairs again to come back.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicesThereSheet(address: String, controlPort: Int, computer: String, onDismiss: () -> Unit) {
    var devices by remember { mutableStateOf<List<DevicesThereRule.Device>?>(null) }
    // A string resource: why there is no list.
    var failure by remember { mutableStateOf<Int?>(null) }
    var asks by remember { mutableIntStateOf(0) }
    var confirming by remember { mutableStateOf<DevicesThereRule.Device?>(null) }
    var removing by remember { mutableStateOf(false) }
    // The last removal's words: a resource, and the device's name when
    // the words name it.
    var said by remember { mutableStateOf<Pair<Int, String?>?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(address, controlPort, asks) {
        failure = null
        // This ask's own id: only the answer naming it is taken (PR #574
        // review I2).
        val asked = DevicesThereRule.newRequest()
        withContext(Dispatchers.IO) { RustCore.devicesThere(address, controlPort, asked) }
            .mapCatching { JSONObject(it) }
            .mapCatching { answer ->
                check(DevicesThereRule.isAnswerTo(asked, answer.optString("request"))) {
                    "an answer to another ask"
                }
                answer
            }
            .onSuccess { answer ->
                if (answer.has("refused")) {
                    // No list kept from before (review N8).
                    devices = null
                    failure = DevicesThereRule.refusal(answer.optString("refused"))
                } else {
                    devices = DevicesThereRule.sorted(parseDevices(answer))
                }
            }
            .onFailure {
                android.util.Log.i("DevicesThereSheet", "No answer: ${it.message}")
                failure = R.string.there_no_answer
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
            Text(stringResource(R.string.there_title, computer), style = MaterialTheme.typography.titleMedium)
            Text(stringResource(R.string.there_hint), style = MaterialTheme.typography.bodySmall)
            said?.let { (words, name) ->
                val text = if (name != null) stringResource(words, name) else stringResource(words)
                Text(text, color = MaterialTheme.colorScheme.primary)
            }
            val shown = devices
            when {
                failure != null -> {
                    Text(stringResource(failure ?: R.string.there_no_answer))
                    TextButton(onClick = { asks++ }) { Text(stringResource(R.string.retry)) }
                }
                shown == null -> Text(stringResource(R.string.there_asking))
                else -> shown.forEach { device ->
                    DeviceRow(device, enabled = !removing) { confirming = device }
                }
            }
        }
    }

    confirming?.let { device ->
        val name = DevicesThereRule.shownName(device)
        AlertDialog(
            onDismissRequest = { confirming = null },
            title = { Text(stringResource(R.string.there_remove_title, name)) },
            text = { Text(stringResource(R.string.there_remove_text, name, computer)) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = null
                    removing = true
                    scope.launch {
                        val asked = DevicesThereRule.newRequest()
                        val answer = withContext(Dispatchers.IO) {
                            RustCore.removeThere(address, controlPort, device.id, asked)
                        }.mapCatching { JSONObject(it) }.getOrNull()
                            ?.takeIf { DevicesThereRule.isAnswerTo(asked, it.optString("request")) }
                        removing = false
                        said = when {
                            answer == null -> R.string.there_no_answer_remove to name
                            answer.has("refused") ->
                                DevicesThereRule.refusal(answer.optString("refused")) to null
                            else -> {
                                val reason = answer.optString("reason").ifEmpty { null }
                                val outcome = DevicesThereRule.outcome(answer.optBoolean("removed"), reason)
                                // Gone either way: removed now, or before (review N8).
                                if (DevicesThereRule.dropsRow(outcome.removed, reason)) {
                                    devices = devices?.let { DevicesThereRule.without(it, device.id) }
                                }
                                if (outcome.removed) {
                                    R.string.there_removed to name
                                } else {
                                    (outcome.reason ?: R.string.there_refused_failed) to null
                                }
                            }
                        }
                        // Listed again, so what shows is what is there now.
                        asks++
                    }
                }) { Text(stringResource(R.string.there_remove), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = {
                TextButton(onClick = { confirming = null }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

@Composable
private fun DeviceRow(device: DevicesThereRule.Device, enabled: Boolean, onRemove: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(
            modifier = Modifier.weight(1f).semantics(mergeDescendants = true) {},
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(DevicesThereRule.shownName(device), style = MaterialTheme.typography.bodyMedium)
            val kinds = DevicesThereRule.kinds(device).map { stringResource(it) }
            Text(
                kinds.joinToString(", "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (DevicesThereRule.removable(device)) {
            TextButton(onClick = onRemove, enabled = enabled) {
                Text(stringResource(R.string.there_remove_more), color = MaterialTheme.colorScheme.error)
            }
        }
    }
}

/** The computer's list: each device that reads; anything else left out. */
private fun parseDevices(answer: JSONObject): List<DevicesThereRule.Device> {
    val list = answer.optJSONArray("devices") ?: return emptyList()
    return (0 until list.length()).mapNotNull { i ->
        val d = list.optJSONObject(i) ?: return@mapNotNull null
        val id = d.optString("id").ifEmpty { return@mapNotNull null }
        DevicesThereRule.Device(
            id = id,
            name = d.optString("name"),
            computer = d.optBoolean("computer"),
            owner = d.optBoolean("owner"),
            guest = d.optBoolean("guest"),
            connected = d.optBoolean("connected"),
            you = d.optBoolean("you"),
        )
    }
}
