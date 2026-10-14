package dev.pocketdesk.android.ui

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
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.discovery.FoundComputer
import dev.pocketdesk.android.discovery.FoundComputers
import dev.pocketdesk.android.discovery.rememberFoundComputers
import dev.pocketdesk.android.doctor.BackgroundLimit
import dev.pocketdesk.android.doctor.BackgroundRunning
import dev.pocketdesk.android.wake.Waker
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Landing screen: the saved-desktop card list. Tapping a card connects;
 * each card carries its own wake (when it can be woken), wake settings and
 * remove actions, and "Add computer" opens the ConnectScreen form.
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
    // N6: a computer being woken, watched until it answers.
    var waking by remember { mutableStateOf<Waking?>(null) }
    var wakingLeft by remember { mutableIntStateOf(0) }
    var wakeSettingsFor by remember { mutableStateOf<String?>(null) }
    // T1: the computer the Connection Doctor is checking.
    var checking by remember { mutableStateOf<HostStore.Host?>(null) }
    // T2: re-read when Home comes back, e.g. from the battery settings.
    Prefs.observedVersion()
    var resumes by remember { mutableIntStateOf(0) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) resumes++
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    // Read once per resume (system services), not on every recomposition.
    val backgroundLimit = remember(resumes) { BackgroundRunning.limit(context) }
    val backgroundFeatures = remember(resumes) { BackgroundRunning.featuresOn(context) }
    val remindBackground =
        BackgroundRunning.shouldRemind(backgroundFeatures, backgroundLimit, Prefs.backgroundReminder(context))
    // A computer whose key changed, to remove and scan again (T1).
    var rescanning by remember { mutableStateOf<HostStore.Host?>(null) }
    // Bumped when wake settings change, so the cards re-read their plans.
    var wakeVersion by remember { mutableIntStateOf(0) }
    // A computer that does not answer Connect, offered "Wake and connect".
    var offeringWake by remember { mutableStateOf<HostStore.Host?>(null) }

    // The computer is awake once its server answers on the control port
    // (whatever address it is saved under). Away, a LAN address never
    // answers, so the wait is shorter and the connection retries by itself.
    LaunchedEffect(waking) {
        val w = waking ?: return@LaunchedEffect
        val limit = if (w.atHome) WAKE_WAIT_HOME_S else WAKE_WAIT_AWAY_S
        val started = System.currentTimeMillis()
        while (true) {
            val elapsed = ((System.currentTimeMillis() - started) / 1_000).toInt()
            if (elapsed >= limit) break
            wakingLeft = limit - elapsed
            val awake = withContext(Dispatchers.IO) { Waker.answers(w.host.address, w.host.controlPort) }
            if (awake) {
                waking = null
                if (w.connect) {
                    onConnect(w.host)
                } else {
                    wakeStatusRes = R.string.wake_awake
                    wakeStatusArg = w.host.address
                }
                return@LaunchedEffect
            }
            delay(2_000)
        }
        waking = null
        if (w.connect) {
            onConnect(w.host)
        } else {
            wakeStatusRes = if (w.atHome) R.string.wake_no_answer else R.string.wake_sent
            wakeStatusArg = w.host.address
        }
    }

    // Send the wake, then watch for the computer to answer.
    fun startWake(host: HostStore.Host, connect: Boolean) {
        wakeStatusRes = null
        scope.launch(Dispatchers.IO) {
            val result = Waker.wake(context, host.address)
            val atHome = Waker.atHome(context, host.address)
            withContext(Dispatchers.Main) {
                if (result.isSuccess) {
                    waking = Waking(host, atHome, connect)
                } else {
                    wakeStatusRes = R.string.wake_failed
                    wakeStatusArg = result.exceptionOrNull()?.message
                }
            }
        }
    }

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
                // Only here, not on the empty Home: the background features
                // need a paired computer to send to.
                if (remindBackground) {
                    item(key = "#background") {
                        BackgroundCard(
                            limit = backgroundLimit,
                            onAllow = { BackgroundRunning.openSettings(context, backgroundLimit) },
                            onDismiss = { Prefs.setBackgroundReminder(context, false) },
                        )
                    }
                }
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
                        hasWake = wakeVersion.let { Waker.canWake(Waker.planFor(context, host.address)) },
                        onConnect = {
                            if (!Prefs.offerWake(context)) {
                                onConnect(host)
                            } else {
                                // A computer that can be woken and does not
                                // answer may be asleep: offer to wake it.
                                scope.launch {
                                    val asleep = withContext(Dispatchers.IO) {
                                        Waker.canWake(Waker.planFor(context, host.address)) &&
                                            !Waker.answersSoon(host.address, host.controlPort)
                                    }
                                    if (asleep) offeringWake = host else onConnect(host)
                                }
                            }
                        },
                        onWake = { startWake(host, Prefs.connectAfterWake(context)) },
                        onWakeSettings = { wakeSettingsFor = host.address },
                        onCheck = { checking = host },
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
            waking?.let { w ->
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 8.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        stringResource(R.string.wake_waking, w.host.address, wakingLeft),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = { waking = null }) { Text(stringResource(R.string.cancel)) }
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

    offeringWake?.let { host ->
        AlertDialog(
            onDismissRequest = { offeringWake = null },
            title = { Text(stringResource(R.string.wake_offer_title, host.address)) },
            text = { Text(stringResource(R.string.wake_offer_message)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        offeringWake = null
                        startWake(host, connect = true)
                    },
                ) { Text(stringResource(R.string.wake_and_connect)) }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        offeringWake = null
                        onConnect(host)
                    },
                ) { Text(stringResource(R.string.connect_anyway)) }
            },
        )
    }

    checking?.let { host ->
        ConnectionDoctorSheet(
            address = host.address,
            streamPort = host.port,
            controlPort = host.controlPort,
            canWake = Waker.canWake(Waker.planFor(context, host.address)),
            onWake = {
                checking = null
                startWake(host, Prefs.connectAfterWake(context))
            },
            onPairAgain = {
                checking = null
                rescanning = host
            },
            onDismiss = { checking = null },
        )
    }

    rescanning?.let { host ->
        AlertDialog(
            onDismissRequest = { rescanning = null },
            title = { Text(stringResource(R.string.doctor_scan_again_title)) },
            text = { Text(stringResource(R.string.doctor_scan_again_text)) },
            confirmButton = {
                TextButton(onClick = {
                    rescanning = null
                    HostStore.remove(context, host.address)
                    hosts = HostStore.hosts(context)
                    // As Remove does: forget the old key, so the code's key
                    // can be trusted for this address, then scan it.
                    scope.launch {
                        withContext(Dispatchers.IO) { RustCore.forgetPeerCertificate(host.address) }
                        onScan()
                    }
                }) { Text(stringResource(R.string.doctor_action_pair)) }
            },
            dismissButton = {
                TextButton(onClick = { rescanning = null }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }

    wakeSettingsFor?.let { address ->
        WakeSettingsDialog(address) {
            wakeSettingsFor = null
            wakeVersion++
        }
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
    onWakeSettings: () -> Unit,
    onCheck: () -> Unit,
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
                TextButton(onClick = onCheck) {
                    Text(stringResource(R.string.doctor_title), style = MaterialTheme.typography.labelMedium)
                }
                TextButton(onClick = onWakeSettings) {
                    Text(stringResource(R.string.wake_settings), style = MaterialTheme.typography.labelMedium)
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

/** A computer being woken: whether the phone is on its network, and whether to connect once it answers. */
private data class Waking(val host: HostStore.Host, val atHome: Boolean, val connect: Boolean)

/** How long the phone watches for a woken computer at home, in seconds. */
private const val WAKE_WAIT_HOME_S = 90

/** How long it waits before connecting from outside, where nothing can be seen, in seconds. */
private const val WAKE_WAIT_AWAY_S = 30

/** T2: Android may stop the features that work in the background. */
@Composable
private fun BackgroundCard(limit: BackgroundLimit, onAllow: () -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val guide = remember { BackgroundRunning.makerGuide(android.os.Build.MANUFACTURER) }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Text(
                stringResource(
                    if (limit == BackgroundLimit.RESTRICTED) R.string.bg_restricted else R.string.bg_optimized,
                ),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                stringResource(R.string.bg_why),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                if (guide != null) {
                    TextButton(onClick = {
                        if (!BackgroundRunning.openGuide(context, guide)) {
                            android.widget.Toast.makeText(context, guide, android.widget.Toast.LENGTH_LONG).show()
                        }
                    }) { Text(stringResource(R.string.bg_maker_guide)) }
                }
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.bg_dont_remind)) }
                TextButton(onClick = onAllow) { Text(stringResource(R.string.bg_allow)) }
            }
        }
    }
}
