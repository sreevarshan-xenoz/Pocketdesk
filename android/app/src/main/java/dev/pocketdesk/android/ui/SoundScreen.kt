package dev.pocketdesk.android.ui

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.automation.PhoneEventBroadcast
import dev.pocketdesk.android.automation.SessionTell
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.service.SessionForegroundService
import dev.pocketdesk.android.stream.DesktopAudio
import dev.pocketdesk.android.ui.RemoteSession.State
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/**
 * The phone as the computer's speaker and mic (N5): a sound session, with
 * no picture or input. The computer's sound plays here (the existing
 * "Desktop sound on this phone" switch), the phone's mic can go there, and
 * the computer's volume and output are a tap away. It runs under the
 * session's foreground service, so it goes on with the screen off.
 */
@Composable
fun SoundScreen(
    address: String,
    port: Int,
    controlPort: Int,
    onExit: () -> Unit,
) {
    val context = LocalContext.current
    // Back ends it, as it ends a session (U2).
    androidx.activity.compose.BackHandler { onExit() }
    var state by remember { mutableStateOf(State.Connecting) }
    var reason by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var shownName by remember(address) { mutableStateOf(HostStore.shownName(context, address)) }

    // Ecosystem 1f: other apps hear it start and end, as a session's.
    val tell = remember(address) { SessionTell("sound") }
    LaunchedEffect(state) {
        val told = when (state) {
            State.Ready -> {
                RustCore.peerName(address)?.let { HostStore.saveClaimedName(context, address, it) }
                shownName = HostStore.shownName(context, address)
                tell.up(Prefs.tellOtherApps(context))
            }
            State.Down, State.TooOld -> tell.lost()
            else -> null
        }
        told?.let { PhoneEventBroadcast.send(context, it.kind, address, it.fields) }
    }
    DisposableEffect(tell) {
        onDispose { tell.left()?.let { PhoneEventBroadcast.send(context, it.kind, address, it.fields) } }
    }

    LaunchedEffect(attempt) {
        state = State.Connecting
        reason = null
        // The computer's sound plays for this attempt, as for a screen
        // session's stream: its own generation, up once admitted and down
        // when it ends, both from here (N5 review: a generation keyed on
        // composition was ended by the old attempt's Down before a Retry
        // could start it, and a screen that is off may not recompose).
        val generation = DesktopAudio.newGeneration(context)
        try {
            val connected =
                withContext(Dispatchers.IO) {
                    RustCore.connectSound(address, port).recoverCatching { lan ->
                        // Left while the LAN dial failed: no internet dial after the screen's stop.
                        coroutineContext.ensureActive()
                        val identity = HostStore.wanIdentity(context, address) ?: throw lan
                        RustCore.connectSoundWan(address, identity).getOrThrow()
                    }
                }
            if (!isActive) {
                if (connected.isSuccess) withContext(NonCancellable + Dispatchers.IO) { RustCore.stopStreaming() }
                return@LaunchedEffect
            }
            if (connected.isFailure) {
                reason = connected.exceptionOrNull()?.message?.let { context.plainErrorText(it) }
                state = State.Down
                return@LaunchedEffect
            }
            state = State.Waiting
            val waitingSince = android.os.SystemClock.elapsedRealtime()
            while (isActive) {
                val (remote, closed) =
                    withContext(Dispatchers.IO) { RemoteSession.parse(RustCore.remoteState()) to RustCore.streamClosedReason }
                val next = RemoteSession.next(state, remote, closed)
                if (next == State.Ready && state != State.Ready) DesktopAudio.streamUp(context, generation)
                state = next
                if (state == State.Waiting && android.os.SystemClock.elapsedRealtime() - waitingSince > WAIT_MS) {
                    state = State.Down
                }
                if (state == State.Down || state == State.TooOld) {
                    reason = closed?.let { context.closeReasonText(it) }
                    break
                }
                delay(POLL_MS)
            }
        } finally {
            DesktopAudio.streamDown(generation)
        }
    }

    // The session's foreground service, so the sound goes on with the
    // screen off or the app in the background; the session ends with the
    // screen. No keep-screen-on: a speaker needs no screen.
    DisposableEffect(Unit) {
        context.startForegroundService(
            Intent(context, SessionForegroundService::class.java).apply {
                action = SessionForegroundService.ACTION_START
                putExtra(SessionForegroundService.EXTRA_ADDRESS, address)
                putExtra(SessionForegroundService.EXTRA_PORT, port)
                putExtra(SessionForegroundService.EXTRA_CONTROL_PORT, controlPort)
            },
        )
        onDispose {
            context.stopService(Intent(context, SessionForegroundService::class.java))
            Thread { RustCore.stopStreaming() }.start()
        }
    }

    // N1: a ring from the computer sounds here too.
    SirenWatcher(context)

    val snackbar = remember { SnackbarHostState() }
    var micErrorRes by remember { mutableStateOf<Int?>(null) }
    val micErrorMessage = micErrorRes?.let { stringResource(R.string.mic_error, stringResource(it)) }
    LaunchedEffect(micErrorMessage) {
        if (micErrorMessage != null) {
            snackbar.showSnackbar(micErrorMessage)
            micErrorRes = null
        }
    }
    val micShare = rememberMicShare(address, port, controlPort) { micErrorRes = it }
    var soundHere by remember { mutableStateOf(Prefs.desktopSound(context)) }
    var showControls by remember { mutableStateOf(false) }

    Box(Modifier.fillMaxSize().safeDrawingPadding()) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(R.string.sound_title, shownName),
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = onExit) { Text(stringResource(R.string.remote_exit)) }
            }
            if (state == State.Ready) {
                Text(
                    stringResource(R.string.sound_on),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
                SettingSwitch(
                    stringResource(R.string.sound_here),
                    stringResource(R.string.sound_here_summary),
                    soundHere,
                    {
                        soundHere = it
                        DesktopAudio.setEnabled(context, it)
                    },
                )
                SettingSwitch(
                    stringResource(R.string.sound_mic),
                    stringResource(R.string.sound_mic_summary),
                    micShare.on,
                    { if (it) micShare.start() else micShare.stop() },
                )
                OutlinedButton(
                    onClick = { showControls = true },
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                ) { Text(stringResource(R.string.sound_controls)) }
            } else {
                SoundStatus(
                    state = state,
                    reason = reason,
                    onRetry = { attempt++ },
                    onExit = onExit,
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                )
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }

    if (showControls) {
        AudioControlSheet(
            address = address,
            controlPort = controlPort,
            onDismiss = { showControls = false },
        )
    }
}

@Composable
private fun SoundStatus(
    state: State,
    reason: String?,
    onRetry: () -> Unit,
    onExit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier.padding(24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        when (state) {
            State.Connecting, State.Waiting -> {
                CircularProgressIndicator()
                Spacer(Modifier.size(16.dp))
                Text(
                    stringResource(if (state == State.Connecting) R.string.remote_connecting else R.string.remote_waiting),
                    textAlign = TextAlign.Center,
                )
            }
            else -> {
                Text(
                    if (state == State.TooOld) {
                        stringResource(R.string.sound_too_old)
                    } else {
                        stringResource(R.string.sound_down, reason ?: stringResource(R.string.remote_no_reason))
                    },
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.size(16.dp))
                Row {
                    Button(onClick = onRetry) { Text(stringResource(R.string.remote_retry)) }
                    Spacer(Modifier.width(12.dp))
                    TextButton(onClick = onExit) { Text(stringResource(R.string.remote_exit)) }
                }
            }
        }
    }
}

private const val POLL_MS = 500L

/** How long it waits to be admitted: longer than the owner's 30 s to decide. */
private const val WAIT_MS = 45_000L
