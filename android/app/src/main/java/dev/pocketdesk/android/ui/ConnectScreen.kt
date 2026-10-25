package dev.pocketdesk.android.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Add-computer form (opened from HomeScreen's "Add computer"): a top bar
 * with a back arrow (U4), the address up front with an example, the URI
 * keyboard and Go, ports + WoL MAC under an Advanced expander, and the
 * auto-connect toggle as a whole row.
 * Connecting is the caller's job (see MainActivity's session state). The
 * optional WoL MAC turns the entered host into a wake *relay* (Tier-2 #12):
 * it emits the magic packet for a sleeping desktop on its LAN.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectScreen(
    initial: HostStore.Host?,
    autoConnect: Boolean,
    onConnect: (address: String, port: Int, controlPort: Int, rememberHost: Boolean) -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    var address by rememberSaveable { mutableStateOf(initial?.address.orEmpty()) }
    var port by rememberSaveable { mutableStateOf((initial?.port ?: HostStore.DEFAULT_STREAMING_PORT).toString()) }
    var controlPort by rememberSaveable {
        mutableStateOf((initial?.controlPort ?: HostStore.DEFAULT_CONTROL_PORT).toString())
    }
    var rememberHost by rememberSaveable { mutableStateOf(autoConnect) }
    var wolMac by rememberSaveable {
        mutableStateOf(HostStore.wolMac(context, initial?.address.orEmpty()))
    }
    var advanced by rememberSaveable { mutableStateOf(initial?.address?.isNotBlank() == true) }
    var wakeStatusRes by remember { mutableStateOf<Int?>(null) }
    var wakeStatusArg by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val portValue = port.toIntOrNull()
    val controlPortValue = controlPort.toIntOrNull()
    val macValue = AddComputerRule.macToKeep(wolMac)

    val canConnect = address.isNotBlank() && portValue != null && controlPortValue != null
    fun connect() {
        // Whatever the auto-connect switch says (AV3: [rememberHost] is
        // it, and a MAC typed with it off was dropped).
        if (macValue != null) {
            HostStore.saveWolMac(context, address.trim(), macValue)
        }
        onConnect(address.trim(), portValue ?: return, controlPortValue ?: return, rememberHost)
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.add_computer)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        LlIcon(LlIcons.ArrowBack, stringResource(R.string.settings_back))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 8.dp),
        ) {
            OutlinedTextField(
                value = address,
                onValueChange = { address = it },
                label = { Text(stringResource(R.string.host_address)) },
                placeholder = { Text(stringResource(R.string.host_address_example)) },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { if (canConnect) connect() }),
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(8.dp))
            TextButton(onClick = { advanced = !advanced }) {
                Text(
                    stringResource(if (advanced) R.string.hide_advanced else R.string.show_advanced),
                    color = MaterialTheme.colorScheme.primary,
                )
            }
            AnimatedVisibility(visible = advanced) {
                Column {
                    OutlinedTextField(
                        value = port,
                        onValueChange = { input ->
                            if (input.all { it.isDigit() } && input.length <= 5) port = input
                        },
                        label = { Text(stringResource(R.string.streaming_port)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = controlPort,
                        onValueChange = { input ->
                            if (input.all { it.isDigit() } && input.length <= 5) controlPort = input
                        },
                        label = { Text(stringResource(R.string.control_port)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = wolMac,
                        onValueChange = { wolMac = it },
                        label = { Text(stringResource(R.string.wol_mac_label)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    OutlinedButton(
                        enabled = address.isNotBlank() && macValue != null,
                        onClick = {
                            val target = macValue ?: return@OutlinedButton
                            HostStore.saveWolMac(context, address.trim(), target)
                            scope.launch {
                                // From this phone, as the computer's wake settings say.
                                val result = withContext(Dispatchers.IO) {
                                    dev.pocketdesk.android.wake.Waker.wake(context, address.trim())
                                }
                                wakeStatusRes =
                                    if (result.isSuccess) R.string.wake_sent else R.string.wake_failed
                                wakeStatusArg = if (result.isSuccess) {
                                    address.trim()
                                } else {
                                    context.plainErrorText(result.exceptionOrNull()?.message)
                                }
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(stringResource(R.string.send_wol))
                    }
                }
            }
            Spacer(Modifier.height(8.dp))
            // One row, one toggle: the label taps it too, and TalkBack reads it once.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .toggleable(value = rememberHost, role = Role.Switch, onValueChange = { rememberHost = it })
                    .padding(vertical = 12.dp),
            ) {
                Text(stringResource(R.string.auto_connect), modifier = Modifier.weight(1f))
                Spacer(Modifier.width(16.dp))
                Switch(checked = rememberHost, onCheckedChange = null)
            }
            Spacer(Modifier.height(16.dp))
            Button(
                enabled = canConnect,
                onClick = { connect() },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(stringResource(R.string.connect))
            }
            val wakeText =
                wakeStatusRes?.let { stringResource(it, wakeStatusArg.orEmpty()) }
            if (wakeText != null) {
                Spacer(Modifier.height(8.dp))
                Text(
                    wakeText,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(16.dp))
        }
    }
}

private val MAC_PATTERN = Regex("^([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$")

/** What the add-computer form keeps of its fields. */
internal object AddComputerRule {
    /** The wake-on-LAN MAC typed, when it is one, to keep with the computer. */
    fun macToKeep(typed: String): String? = MAC_PATTERN.matchEntire(typed.trim())?.value
}
