package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.wake.WakeMethod
import dev.pocketdesk.android.wake.WakePlan
import dev.pocketdesk.android.wake.WakeSettings

/**
 * How this phone wakes the computer at [address] (N6): the method, the
 * computer that relays it, the outside address and port, and the MAC. Each
 * field left blank uses what the computer said about itself.
 */
@Composable
fun WakeSettingsDialog(address: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val saved = remember { HostStore.wakeSettings(context, address) }
    val learned = remember { HostStore.wakeInfo(context, address) }
    // Any saved computer can relay, this one too (an always-on computer
    // saved to wake others behind it, as before N6).
    val others = remember { HostStore.hosts(context).map { it.address } }
    var method by remember { mutableStateOf(saved.method) }
    var relay by remember { mutableStateOf(saved.relay.ifEmpty { others.firstOrNull().orEmpty() }) }
    var outsideHost by remember { mutableStateOf(saved.outsideHost) }
    var outsidePort by remember { mutableStateOf(if (saved.outsidePort > 0) saved.outsidePort.toString() else "") }
    var mac by remember { mutableStateOf(saved.mac) }
    val portOk = WakePlan.validPort(outsidePort)
    val hostOk = WakePlan.validHost(outsideHost)
    val macOk = mac.isBlank() || WakePlan.validMac(mac)
    val relayOk = method != WakeMethod.RELAY || relay.isNotBlank()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.wake_settings)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text(
                    learned?.let {
                        stringResource(
                            R.string.wake_learned,
                            it.mac,
                            stringResource(if (it.wired) R.string.wake_wired else R.string.wake_wifi),
                            if (it.port > 0) it.port.toString() else stringResource(R.string.wake_port_none),
                        )
                    } ?: stringResource(R.string.wake_not_learned),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.wake_method), style = MaterialTheme.typography.labelLarge)
                MethodRow(WakeMethod.AUTO, method, R.string.wake_method_auto, R.string.wake_method_auto_summary) {
                    method = it
                }
                MethodRow(WakeMethod.LAN, method, R.string.wake_method_lan, R.string.wake_method_lan_summary) {
                    method = it
                }
                MethodRow(WakeMethod.RELAY, method, R.string.wake_method_relay, R.string.wake_method_relay_summary) {
                    method = it
                }
                MethodRow(WakeMethod.OFF, method, R.string.wake_method_off, null) { method = it }

                if (method == WakeMethod.RELAY) {
                    Spacer(Modifier.height(4.dp))
                    Text(stringResource(R.string.wake_relay_label), style = MaterialTheme.typography.labelLarge)
                    if (others.isEmpty()) {
                        Text(
                            stringResource(R.string.wake_relay_none),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    others.forEach { other ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .selectable(selected = relay == other, onClick = { relay = other }),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(selected = relay == other, onClick = { relay = other })
                            Text(other)
                        }
                    }
                }
                if (method == WakeMethod.AUTO) {
                    Spacer(Modifier.height(4.dp))
                    OutlinedTextField(
                        value = outsideHost,
                        onValueChange = { outsideHost = it },
                        label = { Text(stringResource(R.string.wake_outside_host)) },
                        isError = !hostOk,
                        supportingText = { if (!hostOk) Text(stringResource(R.string.wake_bad_host)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = outsidePort,
                        onValueChange = { outsidePort = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.wake_outside_port)) },
                        isError = !portOk,
                        supportingText = { if (!portOk) Text(stringResource(R.string.wake_bad_port)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (method != WakeMethod.OFF) {
                    OutlinedTextField(
                        value = mac,
                        onValueChange = { mac = it.trim() },
                        label = { Text(stringResource(R.string.wake_mac_label)) },
                        isError = !macOk,
                        supportingText = { if (!macOk) Text(stringResource(R.string.wake_bad_mac)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = portOk && hostOk && macOk && relayOk,
                onClick = {
                    HostStore.saveWakeSettings(
                        context,
                        address,
                        WakeSettings(
                            method = method,
                            relay = if (method == WakeMethod.RELAY) relay else saved.relay,
                            outsideHost = outsideHost.trim(),
                            outsidePort = outsidePort.trim().toIntOrNull() ?: 0,
                            mac = mac.trim(),
                        ),
                    )
                    onDismiss()
                },
            ) { Text(stringResource(R.string.save)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun MethodRow(
    value: WakeMethod,
    selected: WakeMethod,
    label: Int,
    summary: Int?,
    onSelect: (WakeMethod) -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .selectable(selected = value == selected, onClick = { onSelect(value) })
            .padding(vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = value == selected, onClick = { onSelect(value) })
        Column {
            Text(stringResource(label), style = MaterialTheme.typography.bodyMedium)
            summary?.let {
                Text(
                    stringResource(it),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
