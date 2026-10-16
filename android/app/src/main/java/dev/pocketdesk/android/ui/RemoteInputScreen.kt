package dev.pocketdesk.android.ui

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.service.SessionForegroundService
import dev.pocketdesk.android.stream.keyboard.KeyAction
import dev.pocketdesk.android.stream.keyboard.KeyboardBar
import dev.pocketdesk.android.stream.keyboard.KeyboardSession
import dev.pocketdesk.android.stream.keyboard.KeyboardSinkView
import dev.pocketdesk.android.stream.keyboard.TextSend
import dev.pocketdesk.android.ui.RemoteSession.State
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The phone as a remote (T10i): an input-only session, with no picture or
 * sound. The trackpad and shortcut bar of a session's dock, the keyboard
 * with its bar, and rows of media and presenter keys. Controls show once
 * the computer admitted the session, so nothing typed while its owner
 * decides is injected later.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RemoteInputScreen(
    address: String,
    port: Int,
    controlPort: Int,
    onExit: () -> Unit,
) {
    val context = LocalContext.current
    val haptics = rememberLlHaptics()
    var state by remember { mutableStateOf(State.Connecting) }
    var reason by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }

    LaunchedEffect(attempt) {
        state = State.Connecting
        reason = null
        val connected =
            withContext(Dispatchers.IO) {
                RustCore.connectRemote(address, port).recoverCatching { lan ->
                    // Left while the LAN dial failed: no internet dial after the screen's stop.
                    coroutineContext.ensureActive()
                    val identity = HostStore.wanIdentity(context, address) ?: throw lan
                    RustCore.connectRemoteWan(address, identity).getOrThrow()
                }
            }
        // Left while it connected: the session the dial made has no screen to own it.
        if (!isActive) {
            if (connected.isSuccess) withContext(NonCancellable + Dispatchers.IO) { RustCore.stopStreaming() }
            return@LaunchedEffect
        }
        if (connected.isFailure) {
            reason = connected.exceptionOrNull()?.message
            state = State.Down
            return@LaunchedEffect
        }
        state = State.Waiting
        val waitingSince = android.os.SystemClock.elapsedRealtime()
        while (isActive) {
            val (remote, closed) =
                withContext(Dispatchers.IO) { RemoteSession.parse(RustCore.remoteState()) to RustCore.streamClosedReason }
            state = RemoteSession.next(state, remote, closed)
            // Past the owner's 30 s to decide: the computer is not going to.
            if (state == State.Waiting && android.os.SystemClock.elapsedRealtime() - waitingSince > WAIT_MS) {
                state = State.Down
            }
            if (state == State.Down || state == State.TooOld) {
                reason = closed
                break
            }
            delay(POLL_MS)
        }
    }

    // A session's foreground service and keep-screen-on, and the session
    // ended with the screen.
    DisposableEffect(Unit) {
        context.startForegroundService(
            Intent(context, SessionForegroundService::class.java).apply {
                action = SessionForegroundService.ACTION_START
                putExtra(SessionForegroundService.EXTRA_ADDRESS, address)
                putExtra(SessionForegroundService.EXTRA_PORT, port)
                putExtra(SessionForegroundService.EXTRA_CONTROL_PORT, controlPort)
            },
        )
        val window = context.findActivity()?.window
        val keepOn = dev.pocketdesk.android.Prefs.keepScreenOn(context)
        if (keepOn) window?.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose {
            if (keepOn) window?.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            context.stopService(Intent(context, SessionForegroundService::class.java))
            Thread { RustCore.stopStreaming() }.start()
        }
    }

    // The keyboard, as in a session: the IME types into an invisible sink.
    val keyboardScope = rememberCoroutineScope()
    val keyboard =
        remember(address, port) {
            KeyboardSession(context, keyboardScope) { action ->
                when (action) {
                    is KeyAction.Text -> RustCore.sendText(address, port, action.text)
                    is KeyAction.Tap -> RustCore.tapKey(address, port, action.keyCode)
                    is KeyAction.Combo -> RustCore.hotkey(address, port, action.keyCode, *action.modifiers.toIntArray())
                }
            }
        }
    var keyboardOpen by remember { mutableStateOf(false) }
    var showSendText by remember { mutableStateOf(false) }
    val tooLongText = pluralStringResource(R.plurals.send_text_too_long, TextSend.MAX_CHARS, TextSend.MAX_CHARS)
    var latchVersion by remember { mutableIntStateOf(0) }
    DisposableEffect(keyboard) {
        keyboard.latch.onChange = { latchVersion++ }
        onDispose {
            keyboard.latch.onChange = null
            keyboard.close()
        }
    }
    val imeVisible = WindowInsets.isImeVisible
    var imeSeen by remember { mutableStateOf(false) }
    LaunchedEffect(keyboardOpen, imeVisible) {
        when {
            !keyboardOpen -> imeSeen = false
            imeVisible -> imeSeen = true
            imeSeen -> {
                keyboardOpen = false
                imeSeen = false
                keyboard.close()
            }
        }
    }
    val keyScope = rememberCoroutineScope()
    fun tap(code: Int) {
        haptics.tap()
        keyScope.launch(Dispatchers.IO) { RustCore.tapKey(address, port, code) }
    }

    Box(Modifier.fillMaxSize().safeDrawingPadding()) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(R.string.remote_title, address),
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                if (state == State.Ready) {
                    TextButton(onClick = {
                        keyboardOpen = !keyboardOpen
                        if (keyboardOpen) keyboard.open() else keyboard.close()
                    }) { Text(stringResource(R.string.remote_keyboard)) }
                }
                TextButton(onClick = onExit) { Text(stringResource(R.string.remote_exit)) }
            }
            when (state) {
                State.Ready -> {
                    KeyRow(RemoteSession.media, ::tap)
                    KeyRow(RemoteSession.presenter, ::tap)
                    ControlDock(
                        address = address,
                        port = port,
                        modifier = Modifier.weight(1f).fillMaxWidth().imePadding(),
                    )
                }
                else ->
                    Status(
                        state = state,
                        reason = reason,
                        onRetry = { attempt++ },
                        onExit = onExit,
                        modifier = Modifier.weight(1f).fillMaxWidth(),
                    )
            }
        }

        AndroidView(
            factory = { KeyboardSinkView(it) },
            update = { keyboard.attach(it) },
            modifier = Modifier.size(1.dp).align(Alignment.TopStart),
        )
        if (keyboardOpen && state == State.Ready) {
            KeyboardBar(
                keyboard = keyboard,
                latchVersion = latchVersion,
                haptics = haptics,
                compact = false,
                onHide = {
                    keyboard.close()
                    keyboardOpen = false
                },
                onCompose = { showSendText = true },
                modifier = Modifier.align(Alignment.BottomCenter).imePadding(),
            )
        }
    }

    if (showSendText) {
        SendTextSheet(
            history = keyboard.sentTexts(),
            onSend = { text, pressEnter ->
                if (keyboard.typeText(text, pressEnter).isFailure) tooLongText else null
            },
            onDismiss = { showSendText = false },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun KeyRow(keys: List<RemoteSession.Key>, onTap: (Int) -> Unit) {
    FlowRow(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        keys.forEach { key ->
            val spoken = if (key.spoken != 0) stringResource(key.spoken) else key.label
            FilledTonalButton(
                onClick = { onTap(key.keyCode) },
                modifier = Modifier.semantics { contentDescription = spoken },
            ) { Text(key.label) }
        }
    }
}

@Composable
private fun Status(
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
                        stringResource(R.string.remote_too_old)
                    } else {
                        stringResource(R.string.remote_down, reason ?: stringResource(R.string.remote_no_reason))
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

/** How long a remote waits to be admitted: longer than the owner's 30 s to decide. */
private const val WAIT_MS = 45_000L
