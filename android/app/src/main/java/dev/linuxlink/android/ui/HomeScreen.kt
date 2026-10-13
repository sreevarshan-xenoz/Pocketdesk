package dev.linuxlink.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.linuxlink.android.HostStore
import dev.linuxlink.android.R
import dev.linuxlink.android.bridge.RustCore
import dev.linuxlink.android.discovery.FoundComputer
import dev.linuxlink.android.discovery.FoundComputers
import dev.linuxlink.android.discovery.rememberFoundComputers
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Landing screen: the saved-desktop card list. Tapping a card connects;
 * each card carries its own wake (when a WoL MAC is stored) and remove
 * action, and "Add computer" opens the ConnectScreen form. Replaces the
 * old single-host form as the default landing view (UI refresh).
 */
@Composable
fun HomeScreen(
    onConnect: (HostStore.Host) -> Unit,
    onConnectFound: (HostStore.Host) -> Unit,
    onAdd: () -> Unit,
    onScan: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // The list is re-read on every (re)entry: returning from a session or
    // after add/remove recomposes this screen fresh, and the new last-host
    // ordering must show.
    var hosts by remember { mutableStateOf(HostStore.hosts(context)) }
    var autoConnect by remember { mutableStateOf(HostStore.autoConnect(context)) }
    var removing by remember { mutableStateOf<HostStore.Host?>(null) }
    // Wake status is carried as a @StringRes id + arg and resolved during
    // composition (LocalContext.getString in a coroutine is lint-error:
    // stale configuration) — same discipline as the mic/view-only toasts.
    var wakeStatusRes by remember { mutableStateOf<Int?>(null) }
    var wakeStatusArg by remember { mutableStateOf<String?>(null) }
    // F6: computers announcing themselves on this network, browsed while
    // this screen is in front.
    val found by rememberFoundComputers()
    val network = FoundComputers.merge(found, hosts)
    var confirming by remember { mutableStateOf<FoundComputer?>(null) }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 20.dp, vertical = 16.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column {
                Text(stringResource(R.string.app_name), style = MaterialTheme.typography.headlineMedium)
                Spacer(Modifier.height(4.dp))
                Text(
                    stringResource(R.string.home_subtitle),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            IconButton(onClick = onOpenSettings) {
                LlIcon(LlIcons.Settings, stringResource(R.string.settings_title))
            }
        }
        Spacer(Modifier.height(20.dp))

        if (hosts.isEmpty() && network.unsaved.isEmpty()) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                LlIcon(
                    LlIcons.Desktop,
                    null,
                    size = 72.dp,
                    tint = MaterialTheme.colorScheme.primaryContainer,
                )
                Spacer(Modifier.height(16.dp))
                Text(
                    stringResource(R.string.no_computers),
                    style = MaterialTheme.typography.bodyLarge,
                )
                Spacer(Modifier.height(6.dp))
                Text(
                    stringResource(R.string.home_explainer),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(16.dp))
                Button(onClick = onScan) {
                    Text(stringResource(R.string.scan_pairing_code))
                }
                Spacer(Modifier.height(8.dp))
                OutlinedButton(onClick = onAdd) {
                    Text(stringResource(R.string.add_computer))
                }
            }
        } else {
            LazyColumn(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                if (network.unsaved.isNotEmpty()) {
                    item { SectionLabel(stringResource(R.string.found_on_network)) }
                    items(network.unsaved, key = { "found:${it.address}" }) { computer ->
                        FoundCard(computer, onConnect = { confirming = computer })
                    }
                    if (hosts.isNotEmpty()) {
                        item { SectionLabel(stringResource(R.string.saved_computers)) }
                    }
                }
                items(hosts, key = { it.address }) { host ->
                    HostCard(
                        host = host,
                        online = host.address in network.savedOnline,
                        isAuto = autoConnect && host == hosts.first(),
                        paired = HostStore.pairedDesktopId(context, host.address) != null,
                        hasWake = HostStore.wolMac(context, host.address).isNotBlank(),
                        onConnect = { onConnect(host) },
                        onWake = {
                            val mac = HostStore.wolMac(context, host.address)
                            scope.launch(Dispatchers.IO) {
                                val result =
                                    RustCore.wakeViaRelay(host.address, host.controlPort, mac)
                                withContext(Dispatchers.Main) {
                                    wakeStatusRes =
                                        if (result.isSuccess) R.string.wake_sent else R.string.wake_failed
                                    wakeStatusArg = if (result.isSuccess) {
                                        host.address
                                    } else {
                                        result.exceptionOrNull()?.message
                                    }
                                }
                            }
                        },
                        onRemove = { removing = host },
                    )
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                        TextButton(onClick = onScan) {
                            Text(stringResource(R.string.scan_pairing_code))
                        }
                        TextButton(onClick = onAdd) {
                            Text(stringResource(R.string.add_computer))
                        }
                    }
                }
            }
            wakeStatusRes?.let { res ->
                Text(
                    stringResource(res, wakeStatusArg.orEmpty()),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }
        }
    }

    // Anyone on the network can announce any name: before the PIN goes to
    // an address, say so, and point at the QR code, which pins the computer.
    confirming?.let { computer ->
        AlertDialog(
            onDismissRequest = { confirming = null },
            title = { Text(stringResource(R.string.found_confirm_title)) },
            text = {
                Text(stringResource(R.string.found_confirm_message, computer.name, computer.address))
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirming = null
                        onConnectFound(computer.toHost())
                    },
                ) { Text(stringResource(R.string.found_confirm_connect)) }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        confirming = null
                        onScan()
                    },
                ) { Text(stringResource(R.string.scan_pairing_code)) }
            },
        )
    }

    removing?.let { host ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text(stringResource(R.string.remove_host)) },
            text = { Text(stringResource(R.string.remove_host_message, host.address)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        HostStore.remove(context, host.address)
                        // Forget its certificate too, so a new pairing code
                        // for this address can be trusted again.
                        scope.launch(Dispatchers.IO) { RustCore.forgetPeerCertificate(host.address) }
                        hosts = HostStore.hosts(context)
                        removing = null
                    },
                ) { Text(stringResource(R.string.remove_host)) }
            },
            dismissButton = {
                TextButton(onClick = { removing = null }) {
                    Text(stringResource(R.string.cancel))
                }
            },
        )
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(top = 4.dp),
    )
}

/**
 * A computer found on this network but not saved. Its name is only what it
 * calls itself (mDNS is unauthenticated); a tap connects, and an unpaired
 * computer asks for its PIN.
 */
@Composable
private fun FoundCard(computer: FoundComputer, onConnect: () -> Unit) {
    Card(onClick = onConnect, modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                LlIcon(LlIcons.Desktop, null, size = 22.dp, tint = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.width(10.dp))
                Text(computer.name, style = MaterialTheme.typography.titleMedium)
            }
            Text(
                stringResource(R.string.found_subtitle, computer.address),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun HostCard(
    host: HostStore.Host,
    online: Boolean,
    isAuto: Boolean,
    paired: Boolean,
    hasWake: Boolean,
    onConnect: () -> Unit,
    onWake: () -> Unit,
    onRemove: () -> Unit,
) {
    Card(onClick = onConnect, modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(host.address, style = MaterialTheme.typography.titleMedium)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (online) {
                        Text(
                            stringResource(R.string.found_on_network),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                        Spacer(Modifier.width(8.dp))
                    }
                    if (isAuto) {
                        Text(
                            stringResource(R.string.auto_badge),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                        Spacer(Modifier.width(8.dp))
                    }
                    if (paired) {
                        Text(
                            stringResource(R.string.paired),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            Text(
                stringResource(R.string.host_ports, host.port, host.controlPort),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 2.dp),
                horizontalArrangement = Arrangement.End,
            ) {
                if (hasWake) {
                    TextButton(onClick = onWake) {
                        Text(stringResource(R.string.wake), style = MaterialTheme.typography.labelMedium)
                    }
                }
                TextButton(onClick = onRemove) {
                    Text(
                        stringResource(R.string.remove_host),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        }
    }
}
