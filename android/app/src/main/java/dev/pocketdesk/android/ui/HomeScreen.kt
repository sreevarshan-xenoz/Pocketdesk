package dev.pocketdesk.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarHost
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
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.ComputerName
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.discovery.FoundComputer
import dev.pocketdesk.android.discovery.FoundComputers
import dev.pocketdesk.android.discovery.rememberFoundComputers
import dev.pocketdesk.android.doctor.BackgroundLimit
import dev.pocketdesk.android.doctor.Health
import dev.pocketdesk.android.doctor.HealthCheck
import dev.pocketdesk.android.doctor.HealthProblem
import dev.pocketdesk.android.doctor.HealthReceiver
import dev.pocketdesk.android.doctor.BackgroundRunning
import dev.pocketdesk.android.wake.Waker
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.async
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
    onRemote: (HostStore.Host) -> Unit,
    onSound: (HostStore.Host) -> Unit,
    onCast: (HostStore.Host) -> Unit,
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
    // U3: the card being renamed, and a count that redraws names.
    var renaming by remember { mutableStateOf<HostStore.Host?>(null) }
    var names by remember { mutableIntStateOf(0) }
    LaunchedEffect(hosts) {
        // The names the computers gave themselves on this run's connections.
        if (hosts.count { h -> RustCore.peerName(h.address)?.let { HostStore.saveClaimedName(context, h.address, it) } == true } > 0) {
            names++
        }
    }
    // Wake status is carried as a @StringRes id + arg and resolved during
    // composition (LocalContext.getString in a coroutine is lint-error:
    // stale configuration) — same discipline as the mic/view-only toasts.
    // U2: a wake's result in a snackbar, which is announced and goes by
    // itself (a line at the bottom had stayed, unannounced).
    val snackbar = remember { SnackbarHostState() }
    val resources = LocalResources.current
    var wakeSaid by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(wakeSaid) {
        wakeSaid?.let {
            snackbar.showSnackbar(it)
            wakeSaid = null
        }
    }
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
    // D1: what is on but cannot work, re-read on each resume and when
    // Settings change. A listener that should run and does not is asked to
    // bind again, and judged only once the rebind has had its time.
    val prefsVersion = Prefs.observedVersion()
    var listenerKnown by remember { mutableStateOf(true) }
    LaunchedEffect(resumes) {
        if (HealthCheck.rebindIfStopped(context)) {
            listenerKnown = false
            delay(HealthCheck.REBIND_WAIT_MS)
            listenerKnown = true
        }
    }
    val healthProblems = remember(resumes, listenerKnown, prefsVersion) { HealthCheck.problems(context, listenerKnown) }
    LaunchedEffect(healthProblems) {
        // What an update's check compares with: it tells only of new problems.
        Prefs.setHealthSeen(context, Health.key(healthProblems))
        if (healthProblems.isEmpty()) {
            // Fixed: the notification posted after an update goes too, and
            // "Not now" is forgotten, so the same problem coming back shows.
            HealthReceiver.cancel(context)
            if (Prefs.healthDismissed(context) != null) Prefs.setHealthDismissed(context, null)
        }
    }
    val showHealth = Prefs.healthAlerts(context) && Health.shows(healthProblems, Prefs.healthDismissed(context))
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
    // The card whose tap is still deciding whether its computer is asleep.
    var checkingAddress by remember { mutableStateOf<String?>(null) }

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
                    wakeSaid = resources.getString(R.string.wake_awake, w.host.address)
                }
                return@LaunchedEffect
            }
            delay(2_000)
        }
        waking = null
        if (w.connect) {
            onConnect(w.host)
        } else {
            wakeSaid = resources.getString(
                if (w.atHome) R.string.wake_no_answer else R.string.wake_sent,
                w.host.address,
            )
        }
    }

    // Send the wake, then watch for the computer to answer.
    fun startWake(host: HostStore.Host, connect: Boolean) {
        snackbar.currentSnackbarData?.dismiss()
        scope.launch(Dispatchers.IO) {
            val result = Waker.wake(context, host.address)
            val atHome = Waker.atHome(context, host.address)
            withContext(Dispatchers.Main) {
                if (result.isSuccess) {
                    waking = Waking(host, atHome, connect)
                } else {
                    wakeSaid = resources.getString(
                        R.string.wake_failed,
                        context.plainErrorText(result.exceptionOrNull()?.message),
                    )
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
                if (showHealth) {
                    item(key = "#health") {
                        HealthCard(
                            problems = healthProblems,
                            onFix = { HealthCheck.openFix(context, it) },
                            onTurnOffControl = { Prefs.setControlFromComputer(context, false) },
                            onChooseFolder = onOpenSettings,
                            onNotNow = { Prefs.setHealthDismissed(context, Health.key(healthProblems)) },
                        )
                    }
                }
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
                        name = names.let { HostStore.name(context, host.address) },
                        checking = checkingAddress == host.address,
                        online = host.address in network.savedOnline,
                        isAuto = autoConnect && host == hosts.first(),
                        paired = HostStore.pairedDesktopId(context, host.address) != null,
                        hasWake = wakeVersion.let { Waker.canWake(Waker.planFor(context, host.address)) },
                        onRemote = { onRemote(host) },
                        onSound = { onSound(host) },
                        onCast = { onCast(host) },
                        onConnect = {
                            if (!Prefs.offerWake(context)) {
                                onConnect(host)
                            } else if (checkingAddress == null) {
                                // A computer that can be woken and does not
                                // answer may be asleep: offer to wake it. The
                                // check can take seconds, so the card says so
                                // and takes no second tap meanwhile (U2).
                                checkingAddress = host.address
                                scope.launch {
                                    val asleep = try {
                                        withContext(Dispatchers.IO) {
                                            Waker.canWake(Waker.planFor(context, host.address)) &&
                                                !Waker.answersSoon(host.address, host.controlPort)
                                        } && !answersOverInternetWithin(scope, context, host)
                                    } finally {
                                        checkingAddress = null
                                    }
                                    if (asleep) offeringWake = host else onConnect(host)
                                }
                            }
                        },
                        onWake = { startWake(host, Prefs.connectAfterWake(context)) },
                        onWakeSettings = { wakeSettingsFor = host.address },
                        onCheck = { checking = host },
                        onRemove = { removing = host },
                        onRename = { renaming = host },
                    )
                }
                item {
                    // Wraps: a long label (Tamil's) takes a line of its own
                    // rather than breaking a word per line (U6).
                    FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
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
            SnackbarHost(snackbar)
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

    renaming?.let { host ->
        RenameDialog(
            shown = HostStore.shownName(context, host.address),
            chosen = HostStore.chosenName(context, host.address),
            own = HostStore.claimedName(context, host.address),
            onSave = { typed ->
                HostStore.rename(context, host.address, typed)
                names++
                renaming = null
            },
            onDismiss = { renaming = null },
        )
    }

    removing?.let { host ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text(stringResource(R.string.remove_host)) },
            text = { Text(stringResource(R.string.remove_host_message, HostStore.shownName(context, host.address))) },
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

/**
 * Rename a computer (U3): the owner's name, kept on this phone; blank goes
 * back to the name it gives itself.
 */
@Composable
private fun RenameDialog(
    shown: String,
    chosen: String?,
    own: String?,
    onSave: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var typed by remember { mutableStateOf(chosen.orEmpty()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.rename_title, shown)) },
        text = {
            OutlinedTextField(
                value = typed,
                onValueChange = { typed = it.take(ComputerName.MAX_CHARS * 2) },
                label = { Text(stringResource(R.string.rename_label)) },
                placeholder = own?.let { { Text(it) } },
                supportingText = { Text(stringResource(R.string.rename_hint)) },
                singleLine = true,
            )
        },
        confirmButton = {
            TextButton(onClick = { onSave(typed) }) { Text(stringResource(R.string.save)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
        },
    )
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

/**
 * A saved computer (U4): its name with a status line under it, a filled
 * Connect with Remote and Show screen beside it, and the rest in a menu,
 * Remove last. The whole card still connects, and says so to TalkBack.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun HostCard(
    host: HostStore.Host,
    name: String?,
    checking: Boolean,
    online: Boolean,
    isAuto: Boolean,
    paired: Boolean,
    hasWake: Boolean,
    onConnect: () -> Unit,
    onRemote: () -> Unit,
    onSound: () -> Unit,
    onCast: () -> Unit,
    onWake: () -> Unit,
    onWakeSettings: () -> Unit,
    onCheck: () -> Unit,
    onRemove: () -> Unit,
    onRename: () -> Unit,
) {
    val title = name ?: host.address
    val connectLabel = stringResource(R.string.connect)
    var menu by remember { mutableStateOf(false) }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clip(CardDefaults.shape)
            .clickable(onClickLabel = connectLabel, onClick = onConnect),
    ) {
        Column(modifier = Modifier.padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f).padding(top = 4.dp)) {
                    Text(title, style = MaterialTheme.typography.titleMedium)
                    Text(
                        statusLine(HostStatus.parts(host, name != null, online, paired, isAuto)),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Box {
                    IconButton(onClick = { menu = true }) {
                        LlIcon(LlIcons.MoreVert, stringResource(R.string.card_more, title))
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        val entries = buildList {
                            // The phone as the computer's speaker and mic (N5).
                            add(R.string.sound_button to onSound)
                            if (hasWake) add(R.string.wake to onWake)
                            add(R.string.wake_settings to onWakeSettings)
                            add(R.string.doctor_title to onCheck)
                            add(R.string.card_rename to onRename)
                        }
                        entries.forEach { (label, action) ->
                            DropdownMenuItem(
                                text = { Text(stringResource(label)) },
                                onClick = {
                                    menu = false
                                    action()
                                },
                            )
                        }
                        HorizontalDivider()
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.remove_host), color = MaterialTheme.colorScheme.error) },
                            onClick = {
                                menu = false
                                onRemove()
                            },
                        )
                    }
                }
            }
            if (checking) {
                Text(
                    stringResource(R.string.card_checking),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(top = 4.dp, end = 12.dp),
                )
                androidx.compose.material3.LinearProgressIndicator(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 4.dp, end = 12.dp),
                )
            }
            FlowRow(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp, end = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                // Narrower than Material's 24 dp sides, so the three fit one row on a phone.
                val pad = PaddingValues(horizontal = 16.dp, vertical = 8.dp)
                Button(onClick = onConnect, contentPadding = pad) { Text(connectLabel) }
                // The phone as a remote (T10i): trackpad and keys, no picture.
                OutlinedButton(onClick = onRemote, contentPadding = pad) { Text(stringResource(R.string.remote_button)) }
                // This phone's screen on the computer (T10k).
                OutlinedButton(onClick = onCast, contentPadding = pad) { Text(stringResource(R.string.cast_button)) }
            }
        }
    }
}

/** The card's status line: [HostStatus]'s parts in words, "On this network" in the accent colour. */
@Composable
private fun statusLine(parts: List<HostStatus.Part>): AnnotatedString {
    val accent = MaterialTheme.colorScheme.primary
    val words = parts.map { part ->
        when (part) {
            is HostStatus.Part.Address -> part.address
            HostStatus.Part.OnThisNetwork -> stringResource(R.string.found_on_network)
            HostStatus.Part.Paired -> stringResource(R.string.status_paired)
            HostStatus.Part.NotPaired -> stringResource(R.string.status_not_paired)
            HostStatus.Part.OpensAtLaunch -> stringResource(R.string.status_auto)
            is HostStatus.Part.Ports -> stringResource(R.string.host_ports, part.stream, part.control)
        }
    }
    return buildAnnotatedString {
        parts.forEachIndexed { i, part ->
            if (i > 0) append(" · ")
            if (part == HostStatus.Part.OnThisNetwork) {
                withStyle(SpanStyle(color = accent)) { append(words[i]) }
            } else {
                append(words[i])
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

/** D1: what is on in Pocketdesk but cannot work, each with its fix. */
@Composable
private fun HealthCard(
    problems: List<HealthProblem>,
    onFix: (HealthProblem) -> Unit,
    onTurnOffControl: () -> Unit,
    onChooseFolder: () -> Unit,
    onNotNow: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
    ) {
        Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Text(
                stringResource(R.string.health_title),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            for (problem in problems) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        stringResource(healthText(problem)),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onErrorContainer,
                        modifier = Modifier.weight(1f),
                    )
                    when (problem) {
                        HealthProblem.CONTROL_BLOCKED ->
                            TextButton(onClick = onTurnOffControl) { Text(stringResource(R.string.health_turn_off)) }
                        HealthProblem.DOWNLOAD_FOLDER ->
                            TextButton(onClick = onChooseFolder) { Text(stringResource(R.string.health_choose)) }
                        else -> TextButton(onClick = { onFix(problem) }) { Text(stringResource(R.string.health_fix)) }
                    }
                }
            }
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = onNotNow) { Text(stringResource(R.string.health_not_now)) }
            }
        }
    }
}

private fun healthText(problem: HealthProblem): Int = when (problem) {
    HealthProblem.NOTIFICATION_ACCESS -> R.string.health_notification_access
    HealthProblem.LISTENER_STOPPED -> R.string.health_listener_stopped
    HealthProblem.TEXTS_RECEIVE -> R.string.health_texts_receive
    HealthProblem.TEXTS_SEND -> R.string.health_texts_send
    HealthProblem.CONTACTS -> R.string.health_contacts
    HealthProblem.PHOTOS -> R.string.health_photos
    HealthProblem.APP_NOTIFICATIONS -> R.string.health_app_notifications
    HealthProblem.CONTROL_SERVICE -> R.string.health_control_service
    HealthProblem.CONTROL_BLOCKED -> R.string.health_control_blocked
    HealthProblem.DOWNLOAD_FOLDER -> R.string.health_download_folder
}

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

/** How long the asleep check waits on the internet road (U2: a dial there
 * could take about 30 s, the card waiting all that time). */
private const val INTERNET_CHECK_MS = 10_000L

/**
 * [answersOverInternet], waited on for at most [INTERNET_CHECK_MS]: the dial
 * is a blocking call that cannot be cut short, so it goes on in `scope`, and
 * a link it makes later is kept; the check takes "no answer" meanwhile.
 */
private suspend fun answersOverInternetWithin(
    scope: kotlinx.coroutines.CoroutineScope,
    context: android.content.Context,
    host: HostStore.Host,
): Boolean {
    val dial = scope.async(Dispatchers.IO) { answersOverInternet(context, host) }
    return kotlinx.coroutines.withTimeoutOrNull(INTERNET_CHECK_MS) { dial.await() } ?: false
}

/**
 * Away from home a computer's control port does not answer, though it is
 * awake: with its WAN identity known, try the control link over the
 * internet (T10a) before taking it for asleep. A link that comes up is the
 * session's.
 */
private fun answersOverInternet(context: android.content.Context, host: HostStore.Host): Boolean {
    val wan = HostStore.wanIdentity(context, host.address) ?: return false
    RustCore.connectToPeer(host.address, host.controlPort, wan)
    return RustCore.controlPeer()?.first == host.address
}
