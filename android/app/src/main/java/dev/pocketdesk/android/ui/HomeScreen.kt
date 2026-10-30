package dev.pocketdesk.android.ui

import android.content.Intent
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
import androidx.compose.material3.ButtonDefaults
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
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.core.net.toUri
import dev.pocketdesk.android.ForgetComputer
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
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** How a move of a saved computer to a found address went (O3). */
private enum class MoveOutcome { MOVED, REFUSED, UNREACHED }

/**
 * O3: move [move]'s card when the computer at the new address proves the
 * key its old address was pinned to and the old address no longer answers
 * as it (O3 review: a computer saved by its tailnet or Ethernet address
 * still reached there stays where it is). Blocking.
 */
private fun tryMove(context: android.content.Context, move: FoundComputers.Moved): MoveOutcome {
    val old = move.saved.address
    val new = move.found.address
    when (RustCore.checkIdentityAs(new, move.found.controlPort, old).getOrNull()) {
        "matches" -> {}
        "failed", "closed", null -> return MoveOutcome.UNREACHED
        else -> return MoveOutcome.REFUSED
    }
    if (RustCore.checkIdentity(old, move.saved.controlPort).getOrNull() == "matches") return MoveOutcome.REFUSED
    if (!RustCore.copyPeerCertificate(old, new).getOrDefault(false)) return MoveOutcome.REFUSED
    if (!HostStore.move(context, old, HostStore.Host(new, move.found.streamPort, move.found.controlPort))) {
        // The card went or another took the address meanwhile: the copied pin goes too.
        RustCore.forgetPeerCertificate(new)
        return MoveOutcome.REFUSED
    }
    // Its old pin goes once the card has left it.
    RustCore.forgetPeerCertificate(old)
    return MoveOutcome.MOVED
}

/**
 * T1: whether the computer at [address] now answers as itself on [port] and
 * no longer on [old] (its pinned key is proved; neither port is trusted from
 * the announcement). Blocking.
 */
private fun portMovedAt(address: String, old: Int, port: Int): MoveOutcome {
    when (RustCore.checkIdentity(address, port).getOrNull()) {
        "matches" -> {}
        "failed", "closed", null -> return MoveOutcome.UNREACHED
        else -> return MoveOutcome.REFUSED
    }
    if (RustCore.checkIdentity(address, old).getOrNull() == "matches") return MoveOutcome.REFUSED
    return MoveOutcome.MOVED
}

/**
 * T1: give [change]'s card the control port its computer announces now,
 * once [portMovedAt] says so; then the computer's other cards (saved by a
 * tailnet or Ethernet address mDNS does not announce; T1 review) on the old
 * port, each proved at its own address. Blocking.
 */
private fun tryPorts(change: FoundComputers.Moved, context: android.content.Context): MoveOutcome {
    val old = change.saved.controlPort
    val port = change.found.controlPort
    val outcome = portMovedAt(change.saved.address, old, port)
    if (outcome != MoveOutcome.MOVED) return outcome
    if (!HostStore.setControlPort(context, change.saved.address, port)) return MoveOutcome.REFUSED
    for (other in HostStore.hosts(context)) {
        if (other.address == change.saved.address || other.controlPort != old) continue
        if (HostStore.pairedDesktopId(context, other.address) != change.found.id) continue
        if (portMovedAt(other.address, old, port) == MoveOutcome.MOVED) {
            HostStore.setControlPort(context, other.address, port)
        }
    }
    return MoveOutcome.MOVED
}

/** Where the empty Home sends a phone with no computer yet: the newest release's downloads. */
private const val DESKTOP_APP_URL = "https://github.com/sreevarshan-xenoz/Pocketdesk/releases/latest"

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
    onCamera: (HostStore.Host) -> Unit,
    onPhoneSound: (HostStore.Host) -> Unit,
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
    // D3: the computer whose card opened the clipboard history.
    var historyFor by remember { mutableStateOf<HostStore.Host?>(null) }
    var deckFor by remember { mutableStateOf<HostStore.Host?>(null) }
    var stateFor by remember { mutableStateOf<HostStore.Host?>(null) }
    var peekFor by remember { mutableStateOf<HostStore.Host?>(null) }
    var allowedFor by remember { mutableStateOf<HostStore.Host?>(null) }
    var names by remember { mutableIntStateOf(0) }
    // BB3: the computers that said the phone is not paired, read again every
    // few seconds, as that word comes and goes with no change here.
    var refusers by remember { mutableStateOf(emptySet<String>()) }
    LaunchedEffect(hosts) {
        while (true) {
            refusers = withContext(Dispatchers.IO) {
                hosts.filter { RustCore.unpairedBy(it.address) }.map { it.address }.toSet()
            }
            delay(3_000)
        }
    }
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
    // O3: a saved computer announcing itself at a new address follows its
    // card there once it proves, at the new address, the key its old one
    // was pinned to; each candidate is tried once while Home is up.
    val moves = FoundComputers.moved(found, hosts) { HostStore.pairedDesktopId(context, it) }
    val triedMoves = remember { mutableSetOf<Pair<String, String>>() }
    LaunchedEffect(moves) {
        val movedCards = mutableSetOf<String>()
        for (move in moves) {
            if (move.saved.address in movedCards || (move.saved.address to move.found.address) in triedMoves) continue
            // The whole move or none of it (O3 review: a cancel between the
            // card's move and its old pin's forgetting left that pin).
            val outcome = withContext(NonCancellable + Dispatchers.IO) { tryMove(context, move) }
            // Tried again on a later look only after an answer that may change.
            if (outcome != MoveOutcome.UNREACHED) triedMoves.add(move.saved.address to move.found.address)
            if (outcome == MoveOutcome.MOVED) {
                movedCards += move.saved.address
                hosts = HostStore.hosts(context)
                wakeSaid = resources.getString(
                    R.string.home_computer_moved,
                    HostStore.shownName(context, move.found.address),
                    move.found.address,
                )
            }
        }
    }
    // T1: a saved computer announcing itself at its address on another port
    // (another program holds 1716 there) takes its card to that port once it
    // proves its pinned key on it; each candidate is tried once while Home is up.
    val portChanges = FoundComputers.portMoved(found, hosts) { HostStore.pairedDesktopId(context, it) }
    val triedPorts = remember { mutableSetOf<Pair<String, Int>>() }
    LaunchedEffect(portChanges) {
        val changedCards = mutableSetOf<String>()
        for (change in portChanges) {
            val key = change.saved.address to change.found.controlPort
            if (change.saved.address in changedCards || key in triedPorts) continue
            val outcome = withContext(NonCancellable + Dispatchers.IO) { tryPorts(change, context) }
            if (outcome != MoveOutcome.UNREACHED) triedPorts.add(key)
            if (outcome == MoveOutcome.MOVED) {
                changedCards += change.saved.address
                hosts = HostStore.hosts(context)
                wakeSaid = resources.getString(
                    R.string.home_computer_port,
                    HostStore.shownName(context, change.saved.address),
                    change.found.controlPort,
                )
            }
        }
    }
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
    // Settings change, off the main thread (system services). A listener
    // that should run and does not is asked to bind again, and judged only
    // once the rebind has had its time. Null until the first read, so no
    // card flashes and nothing is recorded before then (D1 review).
    val prefsVersion = Prefs.observedVersion()
    val healthProblems by produceState<List<HealthProblem>?>(null, resumes, prefsVersion) {
        val rebinding = withContext(Dispatchers.IO) { HealthCheck.rebindIfStopped(context) }
        value = withContext(Dispatchers.IO) { HealthCheck.problems(context, listenerKnown = !rebinding) }
        if (rebinding) {
            delay(HealthCheck.REBIND_WAIT_MS)
            value = withContext(Dispatchers.IO) { HealthCheck.problems(context) }
        }
    }
    LaunchedEffect(healthProblems) {
        val problems = healthProblems ?: return@LaunchedEffect
        // What an update's check compares with: it tells only of new problems.
        Prefs.setHealthSeen(context, Health.key(problems))
        if (problems.isEmpty()) {
            // Fixed: the notification posted after an update goes too, and
            // "Not now" is forgotten, so the same problem coming back shows.
            HealthReceiver.cancel(context)
            if (Prefs.healthDismissed(context) != null) Prefs.setHealthDismissed(context, null)
        }
    }
    // UX1: the cards' "Used 2 min ago", read once per list and resume
    // (review N3: not from the preferences at every recomposition), and the
    // clock it is told against, a tick a minute while Home shows.
    val lastUsed = remember(hosts, resumes) { hosts.associate { it.address to HostStore.lastUsed(context, it.address) } }
    val minuteNow by produceState(System.currentTimeMillis(), resumes) {
        while (true) {
            value = System.currentTimeMillis()
            delay(60_000L - value % 60_000L)
        }
    }
    // The Health card's Choose opens the picker itself (E6).
    val chooseFolder = rememberDownloadFolderPicker()
    val showHealth = Prefs.healthAlerts(context) &&
        Health.shows(healthProblems.orEmpty(), Prefs.healthDismissed(context))
    // Read once per resume (system services), not on every recomposition.
    val backgroundLimit = remember(resumes) { BackgroundRunning.limit(context) }
    val backgroundFeatures = remember(resumes) { BackgroundRunning.featuresOn(context) }
    val remindBackground =
        BackgroundRunning.shouldRemind(
            backgroundFeatures,
            backgroundLimit,
            Prefs.backgroundReminder(context),
            // Held until health's first read, so the two do not swap (V2 review).
            healthSaysPaused = healthProblems == null || (showHealth && Health.saysPaused(healthProblems.orEmpty())),
        )
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
                    wakeSaid = resources.getString(R.string.wake_awake, HostStore.shownName(context, w.host.address))
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
                HostStore.shownName(context, w.host.address),
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
                // Not over a Home with nothing saved yet (UX1: it said "Your
                // saved computers" above only a computer found on the network).
                // Its line is kept empty, so the header does not change height
                // as the first computer is saved (PR #575 review N8).
                Spacer(Modifier.height(4.dp))
                Text(
                    if (hosts.isNotEmpty()) stringResource(R.string.home_subtitle) else "",
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
                FirstRunExplainer(centered = true)
                Spacer(Modifier.height(16.dp))
                Button(onClick = onScan) {
                    Text(stringResource(R.string.scan_pairing_code))
                }
                Spacer(Modifier.height(8.dp))
                OutlinedButton(onClick = onAdd) {
                    Text(stringResource(R.string.add_computer))
                }
                Spacer(Modifier.height(8.dp))
                GetDesktopAppButton()
            }
        } else {
            LazyColumn(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                // The cards and the computers found on the network arrive once
                // their checks finish, after the list is drawn, and a list keeps
                // its first item where it was: they had landed above the top
                // edge (S2, on the OPPO). This empty first item is that anchor
                // while the list is at its start, so they show below it; a list
                // scrolled down keeps its place.
                item(key = "#top") {}
                // Only with a computer saved: the background features need
                // one to send to (HomeCards).
                // One system card at a time, Health first (UX1).
                val card = HomeCards.shown(hosts.isNotEmpty(), showHealth, remindBackground)
                if (card == HomeCards.Card.HEALTH) {
                    item(key = "#health") {
                        HealthCard(
                            problems = healthProblems.orEmpty(),
                            onFix = { HealthCheck.openFix(context, it) },
                            onTurnOffControl = { Prefs.setControlFromComputer(context, false) },
                            onTurnOnStayLinked = {
                                Prefs.setStayLinked(context, true)
                                val app = context.applicationContext
                                Thread { runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(app) } }.start()
                            },
                            onChooseFolder = chooseFolder,
                            onOpenSettings = onOpenSettings,
                            onAllowBackground = if (backgroundLimit != BackgroundLimit.NONE) {
                                { BackgroundRunning.openSettings(context, backgroundLimit) }
                            } else {
                                null
                            },
                            onNotNow = { Prefs.setHealthDismissed(context, Health.key(healthProblems.orEmpty())) },
                        )
                    }
                }
                if (card == HomeCards.Card.BACKGROUND) {
                    item(key = "#background") {
                        BackgroundCard(
                            limit = backgroundLimit,
                            onAllow = { BackgroundRunning.openSettings(context, backgroundLimit) },
                            onDismiss = { Prefs.setBackgroundReminder(context, false) },
                        )
                    }
                }
                // UX1: a computer found on the network had hidden the first-run
                // explainer, and with it where the computer's half comes from.
                if (hosts.isEmpty()) {
                    item(key = "#first-run") {
                        Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                            FirstRunExplainer(centered = false)
                            GetDesktopAppButton()
                        }
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
                        unpairedByIt = host.address in refusers,
                        hasWake = wakeVersion.let { Waker.canWake(Waker.planFor(context, host.address)) },
                        lastUsed = lastUsed[host.address],
                        now = minuteNow,
                        onRemote = { onRemote(host) },
                        onSound = { onSound(host) },
                        onCast = { onCast(host) },
                        onCamera = { onCamera(host) },
                        onPhoneSound = { onPhoneSound(host) },
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
                        onHistory = { historyFor = host },
                        onDeck = { deckFor = host },
                        onState = { stateFor = host },
                        onPeek = { peekFor = host },
                        onAllowed = { allowedFor = host },
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
                        stringResource(R.string.wake_waking, HostStore.shownName(context, w.host.address), wakingLeft),
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
            title = { Text(stringResource(R.string.wake_offer_title, HostStore.shownName(context, host.address))) },
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
                    // All that Remove forgets (BC1), its key last, so the
                    // code's key can be trusted for this address; then scan.
                    val forgetting = ForgetComputer.forget(context, scope, host.address)
                    hosts = HostStore.hosts(context)
                    scope.launch {
                        forgetting.join()
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

    // F2: how the computer is doing.
    stateFor?.let { host ->
        StateSheet(
            address = host.address,
            controlPort = host.controlPort,
            computer = HostStore.shownName(context, host.address),
            onDismiss = { stateFor = null },
        )
    }

    // BF2: one still of its screen, with no session.
    peekFor?.let { host ->
        PeekSheet(
            address = host.address,
            controlPort = host.controlPort,
            computer = HostStore.shownName(context, host.address),
            onDismiss = { peekFor = null },
        )
    }

    // M2a: what the computer allows this phone.
    allowedFor?.let { host ->
        AllowedSheet(
            address = host.address,
            controlPort = host.controlPort,
            computer = HostStore.shownName(context, host.address),
            onDismiss = { allowedFor = null },
        )
    }

    // F1: the computer's deck, outside a session.
    deckFor?.let { host ->
        DeckSheet(
            address = host.address,
            controlPort = host.controlPort,
            computer = HostStore.shownName(context, host.address),
            onDismiss = { deckFor = null },
        )
    }

    // D3: the phone's clipboard history, outside a session: a picked copy
    // goes to that computer's clipboard.
    historyFor?.let { host ->
        val shown = HostStore.shownName(context, host.address)
        ClipboardHistorySheet(
            sendTo = shown,
            onDismiss = { historyFor = null },
            onPick = { text ->
                historyFor = null
                scope.launch {
                    // Over the internet road too, as the tile (D3 review).
                    val sent = withContext(Dispatchers.IO) {
                        dev.pocketdesk.android.clip.ClipboardSend.toComputer(context, host, text)
                    }
                    wakeSaid = resources.getString(
                        if (sent) R.string.send_clipboard_sent else R.string.send_clipboard_failed,
                        shown,
                    )
                }
            },
        )
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
                        ForgetComputer.forget(context, scope, host.address)
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

/** What Home says before any computer is saved. */
@Composable
private fun FirstRunExplainer(centered: Boolean) {
    val align = if (centered) TextAlign.Center else TextAlign.Start
    Text(
        stringResource(R.string.no_computers),
        style = MaterialTheme.typography.bodyLarge,
        textAlign = align,
        modifier = Modifier.fillMaxWidth(),
    )
    Spacer(Modifier.height(6.dp))
    Text(
        stringResource(R.string.home_explainer),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = align,
        modifier = Modifier.fillMaxWidth(),
    )
}

/** A first-time user may have only the phone app: where the computer's half comes from. */
@Composable
private fun GetDesktopAppButton() {
    val context = LocalContext.current
    TextButton(onClick = {
        runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, DESKTOP_APP_URL.toUri())) }
    }) {
        Text(stringResource(R.string.home_get_desktop_app))
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
    unpairedByIt: Boolean,
    hasWake: Boolean,
    lastUsed: Long?,
    now: Long,
    onConnect: () -> Unit,
    onRemote: () -> Unit,
    onSound: () -> Unit,
    onCast: () -> Unit,
    onCamera: () -> Unit,
    onPhoneSound: () -> Unit,
    onWake: () -> Unit,
    onWakeSettings: () -> Unit,
    onCheck: () -> Unit,
    onRemove: () -> Unit,
    onRename: () -> Unit,
    onHistory: () -> Unit,
    onDeck: () -> Unit,
    onState: () -> Unit,
    onPeek: () -> Unit,
    onAllowed: () -> Unit,
) {
    val context = LocalContext.current
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
                        statusLine(HostStatus.parts(host, name != null, online, paired, isAuto, unpairedByIt, lastUsed), now),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Box {
                    IconButton(onClick = { menu = true }) {
                        LlIcon(LlIcons.MoreVert, stringResource(R.string.card_more, title))
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        // What it does first, then its upkeep, then Remove (E6).
                        val uses = buildList {
                            // BF2: a glance at its screen, first: the quickest.
                            if (Prefs.peek(context)) add(R.string.card_peek to onPeek)
                            // The phone as the computer's speaker and mic (N5).
                            add(R.string.sound_button to onSound)
                            // F1: the buttons its owner set up there.
                            add(R.string.card_deck to onDeck)
                            // F2: its CPU, memory, disks, battery, uptime.
                            add(R.string.card_state to onState)
                            // F6: the back camera, shown in the computer's window.
                            add(R.string.card_show_camera to onCamera)
                            // M7: the phone's sound there, with no picture (Android 10+).
                            if (dev.pocketdesk.android.cast.CastSoundRule.offeredAlone(android.os.Build.VERSION.SDK_INT)) {
                                add(R.string.card_phone_sound to onPhoneSound)
                            }
                            // D3: an earlier copy, sent to this computer again.
                            add(R.string.card_send_earlier_copy to onHistory)
                            if (hasWake) add(R.string.wake to onWake)
                        }
                        val upkeep = listOf(
                            R.string.wake_settings to onWakeSettings,
                            R.string.doctor_title to onCheck,
                            // M2a: both sides of each feature.
                            R.string.card_allowed to onAllowed,
                            R.string.card_rename to onRename,
                        )
                        uses.forEach { (label, action) ->
                            DropdownMenuItem(
                                text = { Text(stringResource(label)) },
                                onClick = {
                                    menu = false
                                    action()
                                },
                            )
                        }
                        HorizontalDivider()
                        upkeep.forEach { (label, action) ->
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
                // Narrower than Material's 24 dp sides, so the three fit one row
                // on a 360 dp phone (UX1: at 16 dp "Show screen" wrapped to a
                // second row, cut off at the fold).
                val pad = PaddingValues(horizontal = 12.dp, vertical = 8.dp)
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
private fun statusLine(parts: List<HostStatus.Part>, now: Long): AnnotatedString {
    val accent = MaterialTheme.colorScheme.primary
    val error = MaterialTheme.colorScheme.error
    val words = parts.map { part ->
        when (part) {
            is HostStatus.Part.Address -> part.address
            HostStatus.Part.OnThisNetwork -> stringResource(R.string.found_on_network)
            HostStatus.Part.Paired -> stringResource(R.string.status_paired)
            HostStatus.Part.NotPaired -> stringResource(R.string.status_not_paired)
            HostStatus.Part.UnpairedByIt -> stringResource(R.string.status_unpaired_by_it)
            HostStatus.Part.OpensAtLaunch -> stringResource(R.string.status_auto)
            is HostStatus.Part.Ports -> stringResource(R.string.host_ports, part.stream, part.control)
            is HostStatus.Part.LastUsed -> when (val ago = HostStatus.ago(part.at, now)) {
                HostStatus.Ago.JustNow -> stringResource(R.string.status_used_just_now)
                is HostStatus.Ago.Minutes -> pluralStringResource(R.plurals.status_used_minutes, ago.n, ago.n)
                is HostStatus.Ago.Hours -> pluralStringResource(R.plurals.status_used_hours, ago.n, ago.n)
                is HostStatus.Ago.Days -> pluralStringResource(R.plurals.status_used_days, ago.n, ago.n)
                HostStatus.Ago.LongAgo -> stringResource(R.string.status_used_long_ago)
            }
        }
    }
    return buildAnnotatedString {
        parts.forEachIndexed { i, part ->
            if (i > 0) append(" · ")
            if (part == HostStatus.Part.OnThisNetwork) {
                withStyle(SpanStyle(color = accent)) { append(words[i]) }
            } else if (part == HostStatus.Part.UnpairedByIt) {
                withStyle(SpanStyle(color = error)) { append(words[i]) }
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
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun HealthCard(
    problems: List<HealthProblem>,
    onFix: (HealthProblem) -> Unit,
    onTurnOffControl: () -> Unit,
    /** M1: turn "Stay linked with the screen off" on. */
    onTurnOnStayLinked: () -> Unit,
    /** Pocketdesk's Settings: the download folder, screenshots. */
    onChooseFolder: () -> Unit,
    onOpenSettings: () -> Unit,
    /**
     * V2: Android's battery setting, while it limits Pocketdesk: the
     * reminder card that had it steps aside for a pause said here.
     */
    onAllowBackground: (() -> Unit)?,
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
            // On the card's own colour: the theme's primary on red read
            // faintly in the dark (AJ2).
            val onCard = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onErrorContainer)
            problems.forEachIndexed { index, problem ->
                // The sentence takes the card's width and its button goes
                // under it: beside it, a long button left the sentence a
                // column a word wide (S2, on the OPPO).
                Text(
                    stringResource(healthText(problem)),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onErrorContainer,
                    modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                )
                FlowRow(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    // Not now with the last problem's buttons, one row where
                    // they fit, dismissing first (AJ2: on a row of its own it
                    // left the card two rows of single buttons).
                    if (index == problems.lastIndex) {
                        TextButton(onClick = onNotNow, colors = onCard) { Text(stringResource(R.string.health_not_now)) }
                    }
                    val paused = problem == HealthProblem.PAUSED_ANYWAY || problem == HealthProblem.PAUSED_STAY_LINKED_OFF
                    if (paused && onAllowBackground != null) {
                        TextButton(onClick = onAllowBackground, colors = onCard) { Text(stringResource(R.string.bg_allow)) }
                    }
                    when (problem) {
                        HealthProblem.CONTROL_BLOCKED ->
                            TextButton(onClick = onTurnOffControl, colors = onCard) { Text(stringResource(R.string.health_turn_off)) }
                        HealthProblem.DOWNLOAD_FOLDER ->
                            TextButton(onClick = onChooseFolder, colors = onCard) { Text(stringResource(R.string.health_choose)) }
                        HealthProblem.SCREENSHOTS_STOPPED, HealthProblem.CAMERA_STOPPED ->
                            TextButton(onClick = onOpenSettings, colors = onCard) { Text(stringResource(R.string.health_settings)) }
                        HealthProblem.PAUSED_STAY_LINKED_OFF ->
                            TextButton(onClick = onTurnOnStayLinked, colors = onCard) { Text(stringResource(R.string.health_turn_on)) }
                        HealthProblem.PAUSED_ANYWAY ->
                            TextButton(onClick = { onFix(problem) }, colors = onCard) { Text(stringResource(R.string.bg_maker_guide)) }
                        else -> TextButton(onClick = { onFix(problem) }, colors = onCard) { Text(stringResource(R.string.health_fix)) }
                    }
                }
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
    HealthProblem.SCREENSHOTS_STOPPED -> R.string.health_screenshots_stopped
    HealthProblem.CAMERA_STOPPED -> R.string.health_camera_stopped
    HealthProblem.APP_NOTIFICATIONS -> R.string.health_app_notifications
    HealthProblem.CONTROL_SERVICE -> R.string.health_control_service
    HealthProblem.CONTROL_BLOCKED -> R.string.health_control_blocked
    HealthProblem.DOWNLOAD_FOLDER -> R.string.health_download_folder
    HealthProblem.PAUSED_STAY_LINKED_OFF -> R.string.health_paused_stay_linked_off
    HealthProblem.PAUSED_ANYWAY -> R.string.health_paused_anyway
}

/** T2: Android may stop the features that work in the background. */
@OptIn(ExperimentalLayoutApi::class)
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
            // The phone maker's own setting reads as a link under the text,
            // the card's two answers on their row (AJ2: three buttons wrapped
            // into a lone button above the other two).
            if (guide != null) {
                TextButton(
                    onClick = {
                        if (!BackgroundRunning.openGuide(context, guide)) {
                            android.widget.Toast.makeText(context, guide, android.widget.Toast.LENGTH_LONG).show()
                        }
                    },
                    contentPadding = PaddingValues(horizontal = 0.dp),
                ) { Text(stringResource(R.string.bg_maker_guide)) }
            }
            FlowRow(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
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
    // Asked for now: not held back by failures from before (BB5 review).
    RustCore.retryInternetNow(host.address)
    RustCore.connectToPeer(host.address, host.controlPort, wan)
    return RustCore.controlPeer()?.first == host.address
}
