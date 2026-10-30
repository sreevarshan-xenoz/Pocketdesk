package dev.pocketdesk.android.ui

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.automation.PhoneEventBroadcast
import dev.pocketdesk.android.automation.SessionTell
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.media.PhoneMediaEffect
import dev.pocketdesk.android.service.SessionForegroundService
import dev.pocketdesk.android.status.BatteryEffect
import dev.pocketdesk.android.stream.DesktopAudio
import dev.pocketdesk.android.stream.ConnectedGamepad
import dev.pocketdesk.android.stream.GamepadState
import dev.pocketdesk.android.stream.InputMode
import dev.pocketdesk.android.stream.inputModes
import dev.pocketdesk.android.stream.nextMode
import dev.pocketdesk.android.stream.sendGamepadOrdered
import dev.pocketdesk.android.stream.RemoteDesktopView
import dev.pocketdesk.android.stream.ShortcutBar
import dev.pocketdesk.android.stream.StatsHud
import dev.pocketdesk.android.stream.StreamStatus
import dev.pocketdesk.android.stream.StreamTransportKind
import dev.pocketdesk.android.stream.keyboard.KeyAction
import dev.pocketdesk.android.stream.keyboard.KeyboardBar
import dev.pocketdesk.android.stream.keyboard.KeyboardSession
import dev.pocketdesk.android.stream.keyboard.KeyboardSinkView
import dev.pocketdesk.android.stream.keyboard.TextSend
import dev.pocketdesk.android.stream.keyboard.keyboardBarHeight
import dev.pocketdesk.android.ui.theme.Space
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Live remote-desktop session (Tier 1 #5 shell): video surface + input
 * gestures, fading stats HUD, a floating action disc opening the grouped
 * QuickSettingsSheet, the input-mode toggle, shortcut bar, and the
 * foreground-service + keep-screen-on lifecycle around it.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RemoteScreen(
    address: String,
    port: Int,
    controlPort: Int,
    inPictureInPicture: Boolean = false,
    // U2: bumped when the session notification's Lock asks for the question.
    askLock: Int = 0,
    onExit: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // UI refresh: session messages ride one in-app snackbar instead of the
    // old toast spam (they inherit the theme and stay clear of the video).
    val snackbar = remember { SnackbarHostState() }
    val haptics = rememberLlHaptics()
    // F1: the in-session keyboard. The phone's IME types into an invisible
    // sink view; keys and text reach the desktop in order on one worker.
    val keyboardScope = rememberCoroutineScope()
    val keyboard = remember(address, port) {
        KeyboardSession(context, keyboardScope) { action ->
            when (action) {
                is KeyAction.Text -> RustCore.sendText(address, port, action.text)
                is KeyAction.Tap -> RustCore.tapKey(address, port, action.keyCode)
                is KeyAction.Combo ->
                    RustCore.hotkey(address, port, action.keyCode, *action.modifiers.toIntArray())
            }
        }
    }
    var keyboardOpen by remember { mutableStateOf(false) }
    // A short window (landscape) gets the one-row bar, so the stream keeps room.
    val compactBar = with(LocalDensity.current) { LocalWindowInfo.current.containerSize.height.toDp() } <
        COMPACT_BAR_BELOW_DP.dp
    var latchVersion by remember { mutableIntStateOf(0) }
    DisposableEffect(keyboard) {
        keyboard.latch.onChange = { latchVersion++ }
        onDispose {
            keyboard.latch.onChange = null
            // Leaving the session must not leave the IME up over Home.
            keyboard.close()
        }
    }
    // The bar follows the IME: once the keyboard has shown, its going away
    // (Back, or the IME's own hide key) closes the bar too.
    val imeVisible = WindowInsets.isImeVisible
    var imeSeen by remember { mutableStateOf(false) }
    LaunchedEffect(keyboardOpen, imeVisible) {
        when {
            !keyboardOpen -> imeSeen = false
            imeVisible -> imeSeen = true
            imeSeen -> {
                // Back or the IME's own hide key: close fully, so the sink
                // stops taking hardware keys.
                keyboardOpen = false
                imeSeen = false
                keyboard.close()
            }
        }
    }
    var mode by remember { mutableStateOf(InputMode.DirectTouch) }
    // R4 E6: foldable / tablet dual-pane. Auto-follows the hinge posture (or a
    // large window); the "Pane" button cycles Auto → Dual → Single → Auto.
    var paneMode by remember { mutableStateOf(PaneMode.Auto) }
    val activity = remember(context) { context.findActivity() }
    val foldSplit = rememberFoldingSplit(activity)
    var clipboardSync by remember { mutableStateOf(true) }
    var desktopSound by remember { mutableStateOf(Prefs.desktopSound(context)) }
    var showHistory by remember { mutableStateOf(false) }
    // F2: the Send text sheet, and a message to show from a send that failed.
    var showSendText by remember { mutableStateOf(false) }
    // F7b: the computer's files.
    var showFiles by remember { mutableStateOf(false) }
    var showTasks by remember { mutableStateOf(false) }
    var showDeck by remember { mutableStateOf(false) }
    var sendTextNotice by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(sendTextNotice) {
        sendTextNotice?.let {
            snackbar.showSnackbar(it)
            sendTextNotice = null
        }
    }
    // U2: Lock asks first, and Lock and Ring say what happened.
    var confirmLock by remember { mutableStateOf(false) }
    // The session notification's Lock asks here too: only asks made since
    // this screen opened (the count outlives a session).
    val askLockAtStart = remember { askLock }
    LaunchedEffect(askLock) {
        if (askLock > askLockAtStart) confirmLock = true
    }
    val resources = LocalResources.current
    fun ringComputer() {
        scope.launch(Dispatchers.IO) {
            val said = RustCore.sendFindMyDevice(address, controlPort).fold(
                { resources.getString(R.string.ring_sent) },
                { resources.getString(R.string.ring_failed, context.plainErrorText(it.message)) },
            )
            withContext(Dispatchers.Main) { sendTextNotice = said }
        }
    }
    if (confirmLock) {
        LockQuestion(
            name = HostStore.name(context, address),
            onDismiss = { confirmLock = false },
            onLock = {
                scope.launch(Dispatchers.IO) {
                    val said = lockComputer(context, address, controlPort)
                    withContext(Dispatchers.Main) { sendTextNotice = said }
                }
            },
        )
    }
    val tooLongText = pluralStringResource(R.plurals.send_text_too_long, TextSend.MAX_CHARS, TextSend.MAX_CHARS)
    val noClipboardText = stringResource(R.string.phone_clipboard_no_text)
    fun tooLong() = tooLongText
    fun typePhoneClipboard() {
        val clip = phoneClipboardText(context)
        sendTextNotice = when {
            clip == null -> noClipboardText
            keyboard.typeText(clip, pressEnter = false).isFailure -> tooLong()
            else -> null
        }
    }
    // UI refresh: the old 16-button bottom bar became a floating action disc
    // opening this grouped sheet; only the input-mode toggle stayed outside.
    var showQuick by remember { mutableStateOf(false) }
    // E4: the phone as a gamepad, on screen or connected; one state for both.
    var showGamepad by remember { mutableStateOf(false) }
    val gamepad = remember { GamepadState() }
    var gamepadRepeat by remember { mutableStateOf<Job?>(null) }
    var gamepadSending by remember { mutableStateOf<Job?>(null) }
    fun sendGamepad() {
        gamepadSending = sendGamepadOrdered(gamepad.axes.copyOf(), gamepad.buttons)
        // The computer moves the mouse and scrolls once per packet: a stick
        // held off centre is sent again until it is back. A repeat waits
        // while the last send is still going, so a stalled link does not
        // pile up moves to play at once when it comes back (E4 review).
        if (gamepad.moving && gamepadRepeat?.isActive != true) {
            gamepadRepeat = scope.launch {
                while (gamepad.moving) {
                    delay(GamepadState.REPEAT_MS)
                    if (gamepad.moving && gamepadSending?.isActive != true) {
                        gamepadSending = sendGamepadOrdered(gamepad.axes.copyOf(), gamepad.buttons)
                    }
                }
            }
        }
    }
    fun releaseGamepad() {
        gamepadRepeat?.cancel()
        if (gamepad.reset()) sendGamepadOrdered(gamepad.axes.copyOf(), gamepad.buttons)
    }
    // R3#7 single-window streaming: the picked window + the layout box the
    // picker reported (needed to remap direct-touch through the crop).
    var cropWindow by remember { mutableStateOf<DesktopWindow?>(null) }
    var cropScreen by remember { mutableStateOf<IntArray?>(null) }
    var showPicker by remember { mutableStateOf(false) }
    // Shared crop entry: the picker sheet (R3#7) and the workspace HUD's
    // window chips (R4 E1) both pull a window through this. Re-pulling the
    // window already on screen returns to the whole-desktop (monitor) view —
    // the HUD chip row is horizontally scrollable, so "swipe away" is a
    // re-tap rather than a gesture that would fight the scroll.
    val pullWindow: (DesktopWindow, IntArray?) -> Unit = { w, screen ->
        if (cropWindow?.address == w.address) {
            cropWindow = null
            cropScreen = null
            scope.launch(Dispatchers.IO) { RustCore.clearWindowCrop() }
        } else {
            // Crop rect is monitor-local capture space; the video is then
            // re-encoded at the window's resolution.
            cropWindow = w
            cropScreen = screen
            scope.launch(Dispatchers.IO) {
                // The address lets a Hyprland server capture the window
                // itself; the rect serves every other server.
                RustCore.sendWindowCrop(w.localAt[0], w.localAt[1], w.size[0], w.size[1], w.address)
            }
        }
    }
    // R3 Tier-2 #10: which desktop monitor to stream (-1 = server default).
    // Toggling `showStream` tears the view down (surfaceDestroyed stops the
    // session) before recreation, so the new connect can't hit the bridge's
    // already-active check on the same address+port.
    var monitorIndex by remember(address) {
        mutableStateOf(HostStore.monitorIndex(context, address))
    }
    var showStream by remember { mutableStateOf(true) }
    // Each restart (Retry, pairing, a screen switch, a network change) gets
    // a new stream view, so a new surface and a new connection. Toggling
    // showStream off and on alone could land in one frame, which Compose
    // merges into no change: the old view stayed, and nothing reconnected.
    var streamEpoch by remember { mutableIntStateOf(0) }
    // The tallest the top and bottom chrome have been this session, in px:
    // an upright phone keeps that much room above and below the picture, so
    // the chrome (it grows with the details, a relayed link or the window
    // chips) never sits on the desktop, and the room stays as it fades.
    var topChromePx by remember { mutableIntStateOf(0) }
    var bottomChromePx by remember { mutableIntStateOf(0) }
    var showMonitorPicker by remember { mutableStateOf(false) }
    // The computer's screens, for the chrome's screen switcher. Read once per
    // session; the picker in Quick Settings reads them fresh.
    var screens by remember(address) { mutableStateOf<List<DesktopMonitor>>(emptyList()) }
    LaunchedEffect(address, controlPort) {
        withContext(Dispatchers.IO) { RustCore.getMonitors(address, controlPort) }
            .mapCatching(::parseMonitorsPayload)
            .onSuccess { screens = it }
    }
    // Video-link status (LAN/WAN/failed) for the current stream generation.
    var streamStatus by remember {
        mutableStateOf<StreamStatus>(StreamStatus.Connecting)
    }
    // UI refresh: "Connecting" escalates to a reassurance line after a few
    // seconds, and the raw Rust failure behind a Down goes to Logcat only —
    // the user sees the humanized mapping on the status card.
    var stillTrying by remember { mutableStateOf(false) }
    // F5: a desktop that asks before each session holds the stream until its
    // owner answers, so a connected link can show no picture for a while.
    var awaitingPicture by remember { mutableStateOf(true) }
    var showAwaitHint by remember { mutableStateOf(false) }
    LaunchedEffect(streamStatus) {
        stillTrying = false
        (streamStatus as? StreamStatus.Down)?.let {
            android.util.Log.w("RemoteScreen", "stream down: ${it.reason}")
        }
        if (streamStatus is StreamStatus.Connecting) {
            awaitingPicture = true
            delay(6_000)
            stillTrying = true
        }
    }
    // N7: another phone drives this computer, so this phone's input is
    // dropped until it lets go; said on screen so a dead tap is no mystery.
    var otherDrives by remember { mutableStateOf(false) }
    LaunchedEffect(address, controlPort) {
        while (isActive) {
            otherDrives = withContext(Dispatchers.IO) { RustCore.anotherPhoneDrives(address, controlPort) }
            delay(1_000)
        }
    }
    // R4 A1: live link path of the video session ("lan" | "wan_direct" |
    // "wan_relayed" | "wan" | "none"), polled from the bridge while up.
    var linkState by remember { mutableStateOf("none") }
    val status = streamStatus
    // Ecosystem 1f: other apps hear the session start and end.
    val tell = remember(address) { SessionTell("screen") }
    // UX2 (review N10): a session that came up is stamped again as it
    // ends, so a long one reads "Used just now" back on Home.
    val wasUp = remember(address) { java.util.concurrent.atomic.AtomicBoolean(false) }
    LaunchedEffect(status, awaitingPicture) {
        val told = when {
            // Started once the picture comes: a computer that asks before
            // each session holds it until its owner allows it.
            status is StreamStatus.Up && !awaitingPicture -> tell.up(Prefs.tellOtherApps(context))
            status is StreamStatus.Down -> tell.lost()
            else -> null
        }
        told?.let { PhoneEventBroadcast.send(context, it.kind, address, it.fields) }
    }
    DisposableEffect(tell) {
        onDispose { tell.left()?.let { PhoneEventBroadcast.send(context, it.kind, address, it.fields) } }
    }
    DisposableEffect(address) {
        onDispose { if (wasUp.get()) HostStore.markUsed(context, address) }
    }
    LaunchedEffect(status, awaitingPicture) {
        showAwaitHint = false
        if (status is StreamStatus.Up && awaitingPicture) {
            delay(4_000)
            showAwaitHint = true
        }
    }
    LaunchedEffect(status) {
        if (status !is StreamStatus.Up || status.kind != StreamTransportKind.Wan) {
            linkState = "none"
            return@LaunchedEffect
        }
        while (isActive) {
            linkState = runCatching { RustCore.streamingStats().linkState }.getOrDefault("wan")
            delay(1_000)
        }
    }
    // UI refresh: the whole session chrome (HUD, mode row, action disc,
    // shortcut bar, Exit) auto-hides a few seconds after the link comes up
    // — Chrome Remote Desktop style — and comes back on a tap of the
    // transparent top-edge strip (or the HUD itself while it is visible).
    // The timer re-arms whenever the sheets close or the link (re)connects.
    var chromeVisible by remember { mutableStateOf(true) }
    // Tier-2 #11b: PIN pairing state. Unpaired sessions open the sheet —
    // with pairing enforced, the control channel only answers the handshake.
    var pairedServerId by remember(address) { mutableStateOf(HostStore.pairedDesktopId(context, address)) }
    var showPairing by remember { mutableStateOf(false) }
    var pairingMessage by remember { mutableStateOf<String?>(null) }
    // Tier-3 #16: desktop audio control sheet (volume/mute/output routing).
    var showAudio by remember { mutableStateOf(false) }
    // R4 D1: server-enforced view-only mode — while latched the server drops
    // every injectable input packet from this session, video keeps flowing.
    // The flag is per-session server state, so it is re-armed whenever the
    // stream (re)comes up (retry, monitor switch → fresh pipeline).
    var viewOnly by remember { mutableStateOf(false) }
    var viewOnlyError by remember { mutableStateOf<String?>(null) }
    val viewOnlyErrorMessage =
        viewOnlyError?.let { stringResource(R.string.view_only_error, it) }
    LaunchedEffect(viewOnlyErrorMessage) {
        if (viewOnlyErrorMessage != null) {
            snackbar.showSnackbar(viewOnlyErrorMessage)
            viewOnlyError = null
        }
    }
    LaunchedEffect(status) {
        if (viewOnly && status is StreamStatus.Up) {
            withContext(Dispatchers.IO) { RustCore.setViewOnly(true) }
        }
    }
    // R4 A3: while the video link rides an iroh relay the server clamps the
    // encoder bitrate to a bandwidth-friendly cap; this override opts out.
    // Server-side latch, per-session — re-armed on stream (re)connect like
    // view-only. Meaningless on LAN/direct (the clamp isn't engaged there).
    var fullQuality by remember { mutableStateOf(false) }
    var fullQualityError by remember { mutableStateOf<String?>(null) }
    val fullQualityErrorMessage =
        fullQualityError?.let { stringResource(R.string.full_quality_error, it) }
    LaunchedEffect(fullQualityErrorMessage) {
        if (fullQualityErrorMessage != null) {
            snackbar.showSnackbar(fullQualityErrorMessage)
            fullQualityError = null
        }
    }
    LaunchedEffect(status) {
        if (fullQuality && status is StreamStatus.Up) {
            withContext(Dispatchers.IO) { RustCore.setFullQuality(true) }
        }
    }
    // R4 E5: named link-profile preset the HUD cycles through (Auto →
    // Quality → Balanced → Economy). A server-side bitrate ceiling, folded
    // with the A3 relay floor; per-session, so re-armed on (re)connect like
    // the other latches. Auto is the no-op default (never sent on Up).
    var qualityPreset by remember { mutableStateOf(RustCore.PRESET_AUTO) }
    var qualityPresetError by remember { mutableStateOf<String?>(null) }
    val qualityPresetErrorMessage =
        qualityPresetError?.let { stringResource(R.string.quality_preset_error, it) }
    LaunchedEffect(qualityPresetErrorMessage) {
        if (qualityPresetErrorMessage != null) {
            snackbar.showSnackbar(qualityPresetErrorMessage)
            qualityPresetError = null
        }
    }
    LaunchedEffect(status) {
        if (qualityPreset != RustCore.PRESET_AUTO && status is StreamStatus.Up) {
            withContext(Dispatchers.IO) { RustCore.sendQualityPreset(qualityPreset) }
        }
    }
    // Tier-3 #14: decoded video frame size, used to pick the PiP aspect ratio.
    var videoSize by remember { mutableStateOf(androidx.compose.ui.unit.IntSize.Zero) }
    // Tier-3 #15: desktop privacy mode. The server's input grab carries a
    // 10-minute TTL, so an enabled session must keep refreshing it and must
    // release it on exit; a refresh failure means the server auto-released.
    var privacyGrab by remember { mutableStateOf(false) }
    // Snackbar text is resolved during composition (configuration-aware) and
    // fired by this one-shot effect, not from the event callback.
    var privacyError by remember { mutableStateOf<String?>(null) }
    val privacyErrorMessage =
        privacyError?.let { stringResource(R.string.privacy_error, it) }
    LaunchedEffect(privacyErrorMessage) {
        if (privacyErrorMessage != null) {
            snackbar.showSnackbar(privacyErrorMessage)
            privacyError = null
        }
    }

    LaunchedEffect(privacyGrab, address) {
        if (!privacyGrab) return@LaunchedEffect
        while (isActive) {
            // Held only while the computer says so: an answer of
            // `ok: false` (Power turned off, another phone's grab) is a
            // refusal, not a refresh (U2 review).
            val ok = withContext(Dispatchers.IO) {
                RustCore.desktopPrivacy(address, controlPort, "grab").fold(
                    { PrivacyReply.read(it, askedLock = false) == PrivacyOutcome.Done },
                    { false },
                )
            }
            if (!ok) {
                privacyGrab = false
                break
            }
            delay(4 * 60 * 1000)
        }
    }

    DisposableEffect(privacyGrab) {
        onDispose {
            if (privacyGrab) {
                Thread {
                    runCatching { RustCore.desktopPrivacy(address, controlPort, "release") }
                }.start()
            }
        }
    }

    // R4 C2: the server's encoder-fallback ladder can change the wire codec
    // mid-session (e.g. HEVC → software H.264 after a hardware encoder died).
    // The decoder notices on the first re-keyed IDR and reports here; the
    // stream keeps playing, so all we owe the user is the explanation.
    var codecNotice by remember { mutableStateOf<String?>(null) }
    val codecSwitchMessage = codecNotice?.let { stringResource(R.string.codec_switched, it) }
    LaunchedEffect(codecSwitchMessage) {
        if (codecSwitchMessage != null) {
            snackbar.showSnackbar(codecSwitchMessage)
            codecNotice = null
        }
    }

    // R4 E2 phone-mic share: MediaCodec-encodes the mic to Opus and streams
    // it to the desktop's "Pocketdesk Mic" PipeWire source. Independent of
    // view-only — the server routes mic packets before the input drop.
    // Messages are carried as @StringRes ids and resolved during composition
    // (LocalContext.getString in a callback is lint-error: stale
    // configuration); a failure's details go to logcat (U6).
    var micErrorRes by remember { mutableStateOf<Int?>(null) }
    val micErrorMessage = micErrorRes?.let { stringResource(R.string.mic_error, stringResource(it)) }
    LaunchedEffect(micErrorMessage) {
        if (micErrorMessage != null) {
            snackbar.showSnackbar(micErrorMessage)
            micErrorRes = null
        }
    }
    // One mic share for sessions and sound sessions (N5).
    val micShare = rememberMicShare(address, port, controlPort) { micErrorRes = it }

    // Tier-3 #15 remainder: phone-side blackout / pocket mode. The session
    // keeps streaming underneath; the phone shows a black, touch-consuming
    // overlay, its window + video surface go secure (no screenshots,
    // Recents, or screen-record leakage), and brightness drops. Double-tap
    // or back unlocks. (True screen-off is not app-possible without key
    // injection; the FGS partial wake lock keeps the link alive if the user
    // powers down the panel themselves.)
    var blackout by remember { mutableStateOf(false) }

    LaunchedEffect(inPictureInPicture) {
        if (inPictureInPicture) blackout = false
    }

    // UI refresh: the session runs immersive — the system bars are hidden on
    // entry (swipe reveals them transiently, Chrome-Remote-Desktop style)
    // and restored when the session ends or the activity floats into PiP.
    // Keyed on PiP only, so this fires exactly twice per session at most:
    // a mid-session inset flip is the one thing the SurfaceView must not
    // see per recomposition.
    DisposableEffect(inPictureInPicture) {
        val window = activity?.window
        if (window == null || inPictureInPicture) {
            onDispose {}
        } else {
            val controller = androidx.core.view.WindowCompat
                .getInsetsController(window, window.decorView)
            controller.systemBarsBehavior =
                androidx.core.view.WindowInsetsControllerCompat
                    .BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            controller.hide(androidx.core.view.WindowInsetsCompat.Type.systemBars())
            onDispose {
                controller.show(androidx.core.view.WindowInsetsCompat.Type.systemBars())
            }
        }
    }

    DisposableEffect(blackout) {
        val window = context.findActivity()?.window
        fun applySecure(secure: Boolean) {
            if (window == null) return
            if (secure) {
                window.addFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)
            } else {
                window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)
            }
            val lp = window.attributes
            lp.screenBrightness = if (secure) 0.01f else -1f
            window.attributes = lp
        }
        applySecure(blackout)
        onDispose { applySecure(false) }
    }

    // Tier-3 #17: system back / predictive back gesture ends the session
    // cleanly (same path as "Exit" → recompose to ConnectScreen → disposal
    // stops streaming) instead of killing the whole activity. Sheets sit
    // above this in the back stack and consume back themselves; in PiP the
    // system gesture closes the float window, so the handler stands down.
    // Tier-3 #15 remainder: while blackout is on, back just uncovers the
    // phone — it must never drop a session the user can't currently see.
    androidx.activity.compose.BackHandler(enabled = !inPictureInPicture) {
        when {
            blackout -> blackout = false
            // E4: Back closes the gamepad, not the session.
            showGamepad -> {
                showGamepad = false
                releaseGamepad()
            }
            else -> onExit()
        }
    }

    // E4: a controller connected to the phone plays while this session is
    // in front; nothing it held stays held when it goes.
    val controllerOn = Prefs.gamepadController(context)
    DisposableEffect(controllerOn) {
        if (controllerOn) {
            ConnectedGamepad.sink = object : ConnectedGamepad.Sink {
                override fun button(bit: Int, down: Boolean) {
                    scope.launch { if (gamepad.press(bit, down)) sendGamepad() }
                }

                override fun motion(lx: Float, ly: Float, rx: Float, ry: Float, hatX: Float, hatY: Float) {
                    scope.launch {
                        val left = gamepad.stick(left = true, x = lx, y = ly)
                        val right = gamepad.stick(left = false, x = rx, y = ry)
                        val hat = gamepad.hat(hatX, hatY)
                        if (left || right || hat) sendGamepad()
                    }
                }
            }
        }
        // A controller that goes, or this window losing the keyboard (the
        // shade, another app, a call), sends no centred sticks: let go of
        // everything then, or a held stick would keep moving the computer's
        // pointer (E4 review).
        val inputs = context.getSystemService(android.hardware.input.InputManager::class.java)
        val devices = object : android.hardware.input.InputManager.InputDeviceListener {
            override fun onInputDeviceAdded(deviceId: Int) = Unit
            override fun onInputDeviceChanged(deviceId: Int) = Unit
            override fun onInputDeviceRemoved(deviceId: Int) = releaseGamepad()
        }
        inputs?.registerInputDeviceListener(devices, android.os.Handler(android.os.Looper.getMainLooper()))
        onDispose {
            inputs?.unregisterInputDeviceListener(devices)
            ConnectedGamepad.sink = null
            releaseGamepad()
        }
    }
    val windowFocused = LocalWindowInfo.current.isWindowFocused
    LaunchedEffect(windowFocused) {
        if (!windowFocused) releaseGamepad()
    }

    LaunchedEffect(address) {
        val serverId = withContext(Dispatchers.IO) { RustCore.checkPairResult(1L) }
        if (serverId != null) {
            pairedServerId = serverId
            HostStore.savePairedDesktop(context, address, serverId)
            HostStore.keep(context, HostStore.Host(address, port, controlPort))
        } else if (pairedServerId == null) {
            showPairing = true
        }
    }

    // One rebuild at a time: a newer trigger (auto-retry, the network
    // changing, Retry, pairing) replaces one still stopping the old stream.
    var retryJob by remember { mutableStateOf<Job?>(null) }
    fun retryStream() {
        showStream = false
        streamStatus = StreamStatus.Connecting
        retryJob?.cancel()
        retryJob = scope.launch {
            withContext(Dispatchers.IO) { RustCore.stopStreaming() }
            streamEpoch++
            showStream = true
        }
    }

    // R2: a link that went away (the desktop's server restarting, the
    // computer waking) comes back by itself, with backoff, for ten minutes.
    // An end the desktop chose waits for the person (AutoReconnect).
    var reconnectAttempt by remember { mutableIntStateOf(0) }
    var droppedAtMs by remember { mutableLongStateOf(0L) }
    var reconnectInS by remember { mutableStateOf<Int?>(null) }
    // T1: the Connection Doctor, from the error card.
    var showDoctor by remember { mutableStateOf(false) }
    var reconnectGaveUp by remember { mutableStateOf(false) }
    fun restartReconnectWindow() {
        reconnectAttempt = 0
        droppedAtMs = 0L
        reconnectGaveUp = false
    }
    // The backoff starts over only once a picture has played for a while: a
    // stream that connects and then fails (a decoder that cannot take it, a
    // desktop that never sends a frame) must not retry every few seconds
    // forever.
    LaunchedEffect(streamStatus, awaitingPicture) {
        if (streamStatus is StreamStatus.Up && !awaitingPicture) {
            delay(AutoReconnect.HEALTHY_MS)
            restartReconnectWindow()
        }
    }
    LaunchedEffect(streamStatus) {
        reconnectInS = null
        val s = streamStatus
        if (s !is StreamStatus.Down || !Prefs.autoReconnect(context) || !AutoReconnect.retryable(s.reason)) {
            return@LaunchedEffect
        }
        val now = SystemClock.elapsedRealtime()
        if (droppedAtMs == 0L) droppedAtMs = now
        val wait = AutoReconnect.delayMs(reconnectAttempt, now - droppedAtMs)
        if (wait == null) {
            reconnectGaveUp = true
            return@LaunchedEffect
        }
        val due = now + wait
        while (true) {
            val left = due - SystemClock.elapsedRealtime()
            if (left <= 0) break
            reconnectInS = ((left + 999) / 1000).toInt()
            delay(minOf(left, 1_000L))
        }
        reconnectInS = null
        reconnectAttempt++
        retryStream()
    }

    // Tier-3 #13 remainder: Wi-Fi↔LTE roaming. quinn's QUIC migration is
    // ready (server accepts a changing source address), but Android pins
    // the endpoint's UDP socket to the network that was default when it
    // was created, so a migrated path can't actually leave the old radio.
    // The reachable fix is a reconnect on the new default network — cheap
    // for us: fresh QUIC handshake + gap-driven keyframe request. Baseline
    // semantics: registerDefaultNetworkCallback fires onAvailable for the
    // *current* network immediately — that one is swallowed; only a
    // *different* later network triggers a rebind. The cooldown keeps a
    // flapping radio from tearing down a healthy stream.
    var lastNetwork by remember { mutableStateOf<Network?>(null) }
    var lastRebindMs by remember { mutableLongStateOf(0L) }
    // The callback lives across recompositions (keyed on address only), so
    // everything it reads must be a live reference, not the value captured
    // at registration.
    val pipNow = rememberUpdatedState(inPictureInPicture)
    val statusNow = rememberUpdatedState(streamStatus)
    DisposableEffect(address) {
        val handler = Handler(Looper.getMainLooper())
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                handler.post {
                    val previous = lastNetwork
                    lastNetwork = network
                    if (previous != null && previous != network &&
                        !pipNow.value &&
                        SystemClock.elapsedRealtime() - lastRebindMs > 3_000
                    ) {
                        lastRebindMs = SystemClock.elapsedRealtime()
                        restartReconnectWindow()
                        // A connect already under way (the reconnect the
                        // network's return set off) is left to finish:
                        // another would reach the computer as a second
                        // session, captured and encoded until it closed.
                        if (statusNow.value is StreamStatus.Connecting) {
                            android.util.Log.i("RemoteScreen", "default network changed while connecting")
                        } else {
                            android.util.Log.i(
                                "RemoteScreen",
                                "default network changed — rebinding stream",
                            )
                            retryStream()
                        }
                    }
                }
            }
        }
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        runCatching { cm.registerDefaultNetworkCallback(callback) }
        onDispose {
            handler.removeCallbacksAndMessages(null)
            runCatching { cm.unregisterNetworkCallback(callback) }
        }
    }

    // A second screen at this phone's shape (T10j): asked for from the
    // screen picker, taken away when the session goes.
    var addedScreen by remember { mutableStateOf(false) }
    val windowPx = LocalWindowInfo.current.containerSize
    val screenFailed = stringResource(R.string.add_phone_screen_failed)
    DisposableEffect(address, controlPort) {
        onDispose {
            if (addedScreen) Thread { RustCore.removeScreen(address, controlPort) }.start()
        }
    }

    /** Stream screen [index]; [remember] keeps it as this computer's screen for next time. */
    fun switchMonitor(index: Int, remember: Boolean = true) {
        showMonitorPicker = false
        if (index == monitorIndex) return
        // An added screen (T10j) is gone after the session: never the default.
        if (remember) HostStore.saveMonitorIndex(context, address, index)
        cropWindow = null
        cropScreen = null
        monitorIndex = index
        streamStatus = StreamStatus.Connecting
        showStream = false
        scope.launch {
            withContext(Dispatchers.IO) { RustCore.stopStreaming() }
            streamEpoch++
            showStream = true
        }
    }

    // Session-state toggles, shared by the quick-settings sheet. Each keeps
    // the existing error-toast path (viewOnlyError / privacyError / ...).
    fun applyViewOnly(next: Boolean) {
        scope.launch(Dispatchers.IO) {
            val result = RustCore.setViewOnly(next)
            withContext(Dispatchers.Main) {
                if (result.isSuccess) viewOnly = next else viewOnlyError = context.plainErrorText(result.exceptionOrNull()?.message)
            }
        }
    }
    fun applyPrivacy(next: Boolean) {
        scope.launch(Dispatchers.IO) {
            val result =
                RustCore.desktopPrivacy(address, controlPort, if (next) "grab" else "release")
            val outcome = result.fold(
                { PrivacyReply.read(it, askedLock = false) },
                { PrivacyOutcome.Refused(context.plainErrorText(it.message)) },
            )
            withContext(Dispatchers.Main) {
                when (outcome) {
                    PrivacyOutcome.Done -> privacyGrab = next
                    is PrivacyOutcome.Refused ->
                        privacyError = context.plainErrorText(outcome.why)
                }
            }
        }
    }
    // Sent again even when unchanged: a tap on the chosen one retries a
    // send that failed (U4 review).
    fun setQuality(next: Int) {
        qualityPreset = next
        scope.launch(Dispatchers.IO) {
            RustCore.sendQualityPreset(next).onFailure { qualityPresetError = context.plainErrorText(it.message) }
        }
    }
    // E3: the pen, where the computer says it takes one, on the ready stream
    // it sends as the session is admitted.
    var penOffered by remember(address) { mutableStateOf(false) }
    fun toggleMode() {
        mode = nextMode(mode, inputModes(penOffered))
    }

    // Each time the stream comes up, the control link is dialed at once:
    // its backoff grew while the computer was away, and without it a
    // session that looks live cannot take a file or a push.
    var streamUps by remember(address) { mutableIntStateOf(0) }
    LaunchedEffect(streamStatus is StreamStatus.Up) {
        if (streamStatus is StreamStatus.Up) {
            streamUps++
            // U3: the name the computer gave itself on this connection.
            RustCore.peerName(address)?.let { HostStore.saveClaimedName(context, address, it) }
            // UX1: its card's "Used just now".
            HostStore.markUsed(context, address)
            wasUp.set(true)
        }
    }
    LaunchedEffect(streamUps) {
        if (streamUps == 0) return@LaunchedEffect
        var offered = false
        for (attempt in 0 until 10) {
            if (withContext(Dispatchers.IO) { RustCore.penSupported() }) {
                offered = true
                break
            }
            delay(500)
        }
        penOffered = offered
        // A computer that does not take a pen (an older one, met again).
        if (!offered && mode == InputMode.Pen) mode = InputMode.DirectTouch
    }
    ControlLinkEffect(address, controlPort, streamUps)
    ClipboardSyncEffect(address, controlPort, enabled = clipboardSync)
    SirenWatcher(context)
    DesktopNotificationRelay(address = address, controlPort = controlPort)
    FileOffersEffect(address = address, controlPort = controlPort)
    LinksEffect(address = address)
    CommandsEffect(address = address)
    DialsEffect(address = address)
    BatteryEffect(address = address, controlPort = controlPort)
    PhoneMediaEffect(address = address, controlPort = controlPort)

    // R1 stage 3: cache the desktop's iroh WAN identity while the control
    // channel is up (the server pushes it on register and re-announces every
    // 30 s), so later sessions can dial it from off-LAN.
    LaunchedEffect(address) {
        var cached: String? = null
        while (isActive) {
            // Only this computer's (AQ3): the link may be to another one.
            val identity = withContext(Dispatchers.IO) { RustCore.getWanIdentity(address, controlPort) }
            if (identity != null && identity != cached) {
                cached = identity
                HostStore.saveWanIdentity(context, address, identity)
            }
            // How to wake it later, from this phone (N6).
            withContext(Dispatchers.IO) { RustCore.wakeInfo(address, controlPort) }
                ?.let { HostStore.saveWakeInfo(context, address, it) }
            delay(5_000)
        }
    }

    DisposableEffect(Unit) {
        val start = Intent(context, SessionForegroundService::class.java).apply {
            action = SessionForegroundService.ACTION_START
            putExtra(SessionForegroundService.EXTRA_ADDRESS, address)
            putExtra(SessionForegroundService.EXTRA_PORT, port)
            putExtra(SessionForegroundService.EXTRA_CONTROL_PORT, controlPort)
        }
        context.startForegroundService(start)
        val window = context.findActivity()?.window
        val keepOn = dev.pocketdesk.android.Prefs.keepScreenOn(context)
        if (keepOn) {
            window?.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
        onDispose {
            if (keepOn) {
                window?.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            context.stopService(Intent(context, SessionForegroundService::class.java))
        }
    }

    val metrics = remember { context.resources.displayMetrics }

    // UI refresh: transparent Scaffold purely for the snackbar host.
    // contentWindowInsets = 0 so the video Box keeps the full-bleed size —
    // Scaffold's default padding would resize the SurfaceView mid-session.
    Scaffold(
        modifier = Modifier.fillMaxSize(),
        containerColor = Color.Transparent,
        contentWindowInsets = WindowInsets(0),
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxSize()
            .padding(padding)
            // With the keyboard open the session sits above it (spec K9).
            .then(if (keyboardOpen) Modifier.imePadding() else Modifier)
            .background(androidx.compose.ui.graphics.Color.Black),
    ) {
        // A hinge (FoldingFeature) or a large window splits the session into a
        // stream pane and a control dock (native trackpad + shortcut bar), so
        // the finger never covers the remote view — RustDesk's tablet UI is
        // just a stretched phone layout; we own both endpoints and can split.
        // T9b: a phone held upright splits too, the stream on top sized to
        // the desktop's shape and the controls below (PaneRule).
        // Decided on the window, not this box: the box shrinks while the
        // on-screen keyboard is up, and the layout must not flip under it.
        val window = LocalWindowInfo.current.containerSize
        val density = LocalDensity.current
        val hinge = foldSplit != PaneSplit.None
        val split = PaneRule.resolve(
            paneMode,
            foldSplit,
            widthDp = window.width / density.density,
            heightDp = window.height / density.density,
            pip = inPictureInPicture,
            // A phone on its side is wide, not a tablet (C5).
            smallestWidthDp = androidx.compose.ui.platform.LocalConfiguration.current.smallestScreenWidthDp.toFloat(),
            // Direct touch and the pen touch the picture: no trackpad under it.
            touchesStream = mode != InputMode.Trackpad,
        )
        val dualPane = split != PaneSplit.None
        // An upright phone's stream pane keeps room above and below the
        // picture, so its chrome is not drawn over the desktop: stacked over
        // the trackpad, or alone (Direct touch, Pen).
        val upright = !inPictureInPicture && window.height >= window.width * PaneRule.PORTRAIT_RATIO
        val banded = upright && ((split == PaneSplit.Horizontal && !hinge) || split == PaneSplit.None)
        val topBandPx = maxOf(with(density) { PaneRule.TOP_BAND_DP.dp.toPx() }, topChromePx.toFloat())
        val bottomBandPx = maxOf(with(density) { PaneRule.BOTTOM_BAND_DP.dp.toPx() }, bottomChromePx.toFloat())
        val vertical = split == PaneSplit.Vertical
        val streamShare = PaneRule.streamFraction(
            split,
            hinge = hinge,
            widthPx = window.width.toFloat(),
            heightPx = window.height.toFloat(),
            videoAspect = if (videoSize.height > 0) videoSize.width.toFloat() / videoSize.height else 0f,
            // Up and showing a picture: until then the card or the wait hint
            // keeps the whole screen, so the pane changes size once, not thrice.
            up = status is StreamStatus.Up && !awaitingPicture,
            bandsPx = topBandPx + bottomBandPx,
        )
        val streamHeight = with(density) { (window.height * streamShare).toDp() }.coerceAtMost(maxHeight)
        // Only the stream Box's size changes across a fold, so the SurfaceView
        // subtree is reused — the decode session is not torn down on a hinge.
        val streamMod = when {
            !dualPane -> Modifier.fillMaxSize()
            vertical -> Modifier.fillMaxHeight().fillMaxWidth(0.5f).align(Alignment.CenterStart)
            else -> Modifier.fillMaxWidth().height(streamHeight).align(Alignment.TopCenter)
        }
        // Any touch on the stream brings the chrome back. Observing on the
        // Initial pass means the parent sees the down before the video's own
        // gesture handlers and never consumes it, so the tap still reaches the
        // desktop — no swallowed input, no invisible edge strip to hunt for.
        Box(
            modifier = streamMod
                .then(
                    // The bar runs along the window's bottom: it covers the
                    // stream unless the stream sits on top (PaneRule).
                    if (keyboardOpen && PaneRule.streamUnderKeyboardBar(split)) {
                        Modifier.padding(bottom = keyboardBarHeight(compactBar))
                    } else {
                        Modifier
                    },
                )
                .pointerInput(Unit) {
                awaitEachGesture {
                    while (true) {
                        val event = awaitPointerEvent(PointerEventPass.Initial)
                        if (event.changes.any { it.pressed }) chromeVisible = true
                        // A touch on the desktop may move its caret: the
                        // keyboard's idea of the field is stale.
                        if (keyboardOpen && event.changes.any { it.pressed && !it.previousPressed }) {
                            keyboard.controller.contextLost()
                        }
                        if (event.changes.all { !it.pressed }) break
                    }
                }
            },
        ) {
        if (showStream) key(streamEpoch) {
            RemoteDesktopView(
                address = address,
                port = port,
                width = metrics.widthPixels,
                height = metrics.heightPixels,
                inputMode = mode,
                mapping = cropWindow?.desktopMapping(cropScreen),
                monitorIndex = monitorIndex,
                onStatus = { streamStatus = it },
                onCodecChanged = { label -> codecNotice = label },
                onVideoSizeChanged = { w, h ->
                    videoSize = androidx.compose.ui.unit.IntSize(w, h)
                    awaitingPicture = false
                },
                secure = blackout,
                modifier = if (banded) {
                    Modifier.fillMaxSize().padding(
                        top = with(density) { topBandPx.toDp() },
                        bottom = with(density) { bottomBandPx.toDp() },
                    )
                } else {
                    Modifier.fillMaxSize()
                },
            )
        }

        // UI refresh: chrome auto-hide timer. Sticky (never fades) while any
        // sheet is open or a side-by-side (or hinged) dock is on screen — that
        // dock is a permanent control surface, and fading the stream chrome
        // next to it would look broken. Nor on an upright phone, where the
        // chrome sits in the bands off the picture (PaneRule.chromeStays).
        val chromeStaysHere = PaneRule.chromeStays(split, hinge, banded)
        val chromeSticky = showQuick || showPicker || showMonitorPicker ||
            showPairing || showAudio || showHistory || chromeStaysHere ||
            keyboardOpen || showSendText || showFiles
        // Where the chrome stays, it is shown: it may have faded before (on
        // its side, or in picture-in-picture), and staying hidden would leave
        // empty bands and no controls.
        LaunchedEffect(chromeStaysHere) {
            if (chromeStaysHere) chromeVisible = true
        }
        LaunchedEffect(chromeVisible, status, chromeSticky) {
            if (chromeVisible && !chromeSticky && status is StreamStatus.Up) {
                delay(6_000)
                chromeVisible = false
            }
        }

        // Every chrome element is hidden while the activity is in picture-in-picture:
        // the window is thumb-sized and touch input there goes to the system, not us.
        if (!inPictureInPicture) {
            if (status !is StreamStatus.Up) {
                // UI refresh: the small top-center chip became a centered
                // card — spinner + staged copy while connecting, a
                // humanized reason with Retry / Pair-again / Exit on
                // failure (raw reason goes to Logcat, never to the user).
                Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) {
                    Surface(
                        shape = Space.sheet,
                        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.95f),
                        contentColor = MaterialTheme.colorScheme.onSurface,
                        tonalElevation = 6.dp,
                        modifier = Modifier.padding(horizontal = 40.dp),
                    ) {
                        Column(
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                            modifier = Modifier.padding(24.dp),
                        ) {
                            when (val s = status) {
                                is StreamStatus.Connecting -> {
                                    CircularProgressIndicator()
                                    Text(
                                        stringResource(R.string.connecting),
                                        style = MaterialTheme.typography.titleMedium,
                                    )
                                    if (stillTrying) {
                                        Text(
                                            stringResource(R.string.connecting_still),
                                            style = MaterialTheme.typography.bodyMedium,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                    }
                                }

                                is StreamStatus.Down -> {
                                    LlIcon(
                                        LlIcons.Warning,
                                        null,
                                        tint = MaterialTheme.colorScheme.error,
                                        size = 34.dp,
                                    )
                                    Text(
                                        stringResource(humanizeError(s.reason)),
                                        style = MaterialTheme.typography.titleMedium,
                                        textAlign = TextAlign.Center,
                                    )
                                    reconnectInS?.let { seconds ->
                                        Text(
                                            stringResource(R.string.reconnecting_in, seconds),
                                            style = MaterialTheme.typography.bodyMedium,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                    }
                                    if (reconnectGaveUp) {
                                        Text(
                                            stringResource(R.string.reconnect_gave_up),
                                            style = MaterialTheme.typography.bodyMedium,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                    }
                                    Button(onClick = {
                                        restartReconnectWindow()
                                        retryStream()
                                    }) {
                                        Text(stringResource(R.string.retry))
                                    }
                                    TextButton(onClick = { showDoctor = true }) {
                                        Text(stringResource(R.string.doctor_title))
                                    }
                                    if (errorSuggestsRepair(s.reason)) {
                                        TextButton(
                                            onClick = {
                                                pairingMessage = null
                                                showPairing = true
                                            },
                                        ) {
                                            Text(stringResource(R.string.repair_pairing))
                                        }
                                    }
                                    TextButton(onClick = onExit) {
                                        Text(stringResource(R.string.exit))
                                    }
                                }

                                else -> Unit
                            }
                        }
                    }
                }
            } else if (showAwaitHint) {
                Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) {
                    Surface(
                        shape = Space.sheet,
                        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.95f),
                        contentColor = MaterialTheme.colorScheme.onSurface,
                        tonalElevation = 6.dp,
                        modifier = Modifier.padding(horizontal = 40.dp),
                    ) {
                        Column(
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                            modifier = Modifier.padding(24.dp),
                        ) {
                            CircularProgressIndicator()
                            Text(
                                stringResource(R.string.waiting_for_picture),
                                style = MaterialTheme.typography.titleMedium,
                                textAlign = TextAlign.Center,
                            )
                            Text(
                                stringResource(R.string.waiting_for_approval),
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                textAlign = TextAlign.Center,
                            )
                        }
                    }
                }
            }

            // Stays while chrome fades: it explains taps that do nothing.
            if (otherDrives && Prefs.showDriving(context)) {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .windowInsetsPadding(WindowInsets.systemBars),
                    contentAlignment = Alignment.TopCenter,
                ) {
                    Text(
                        stringResource(R.string.driving_other),
                        color = Color.White,
                        style = MaterialTheme.typography.labelMedium,
                        modifier = Modifier
                            .padding(top = 8.dp)
                            .background(Color.Black.copy(alpha = 0.6f), RoundedCornerShape(8.dp))
                            .padding(horizontal = 10.dp, vertical = 4.dp),
                    )
                }
            }
            val baseDensity = LocalDensity.current
            // Replaced only above the cap: the system's own density keeps
            // Android 14's non-linear text scaling below it (C5 review).
            val chromeDensity = remember(baseDensity) {
                if (ChromeText.capped(baseDensity.fontScale)) {
                    androidx.compose.ui.unit.Density(baseDensity.density, ChromeText.scale(baseDensity.fontScale))
                } else {
                    baseDensity
                }
            }
            // Top chrome: HUDs + WAN badge + Exit, as one fading slide-down
            // group. AnimatedVisibility removes the subtree when hidden, so
            // invisible chrome never eats a desktop tap.
            AnimatedVisibility(
                visible = chromeVisible,
                enter = fadeIn(tween(160)) + slideInVertically(tween(240)) { -it / 3 },
                exit = fadeOut(tween(240)) + slideOutVertically(tween(240)) { -it / 3 },
                modifier = Modifier
                    .fillMaxSize()
                    .windowInsetsPadding(WindowInsets.systemBars),
            ) {
                // C5: the chrome's text stops at 1.3x (ChromeText).
                androidx.compose.runtime.CompositionLocalProvider(LocalDensity provides chromeDensity) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        // Clear of Exit (48 dp at the top end): the details run
                        // wider in some languages (U6, found in Tamil).
                        Column(
                            modifier = Modifier
                                .onSizeChanged { if (it.height > topChromePx) topChromePx = it.height }
                                .padding(start = 8.dp, top = 8.dp, bottom = 8.dp, end = 72.dp),
                        ) {
                            StatsHud(
                                address = address,
                                controlPort = controlPort,
                                details = dev.pocketdesk.android.Prefs.connectionDetails(context),
                                live = status is StreamStatus.Up,
                            )
                            // A live WAN link keeps a small badge reporting *how* it
                            // is connected (R4 A1) — punched-direct vs relayed, since
                            // relayed is a normal first-class state iroh keeps trying
                            // to upgrade, not an error. It sits under the stats panel:
                            // centred over the top edge it ran into the HUD caption
                            // whenever the stream pane was narrow.
                            if (status is StreamStatus.Up && status.kind == StreamTransportKind.Wan) {
                                val label = when (linkState) {
                                    "wan_direct" -> stringResource(R.string.wan_link_direct)
                                    "wan_relayed" -> stringResource(R.string.wan_link_relayed)
                                    else -> stringResource(R.string.wan_link_punching)
                                }
                                Column(
                                    modifier = Modifier
                                        .padding(top = 6.dp)
                                        .background(
                                            Color.Black.copy(alpha = 0.55f),
                                            RoundedCornerShape(8.dp),
                                        )
                                        .padding(horizontal = 8.dp, vertical = 4.dp),
                                ) {
                                    Text(
                                        label,
                                        color = Color.White,
                                        style = MaterialTheme.typography.labelSmall,
                                    )
                                    // One-tap escape hatch from the server's relay bitrate
                                    // floor (R4 A3): relaying is shared bandwidth, but the
                                    // user may still want every bit of it.
                                    if (linkState == "wan_relayed") {
                                        TextButton(
                                            onClick = {
                                                val next = !fullQuality
                                                fullQuality = next
                                                scope.launch(Dispatchers.IO) {
                                                    RustCore.setFullQuality(next)
                                                        .onFailure { fullQualityError = context.plainErrorText(it.message) }
                                                }
                                            },
                                        ) {
                                            val fqLabel = if (fullQuality) {
                                                stringResource(R.string.full_quality_on)
                                            } else {
                                                stringResource(R.string.full_quality_off)
                                            }
                                            Text(
                                                fqLabel,
                                                color = if (fullQuality) Color(0xFFFFC080) else Color.White,
                                                style = MaterialTheme.typography.labelSmall,
                                            )
                                        }
                                    }
                                }
                            }
    
                            WorkspaceHud(
                                address = address,
                                port = port,
                                modifier = Modifier.padding(top = 6.dp),
                                croppedAddress = cropWindow?.address,
                                onPullWindow = pullWindow,
                            )
                        }
    
                        // Exit rides a scrim of its own: a bare white glyph
                        // disappears over a bright desktop and reads as part of
                        // the remote image rather than as the one control that
                        // must be findable. The dark fill is deliberate here —
                        // unlike the HUD readouts, this plate has to survive being
                        // drawn on top of anything.
                        Surface(
                            onClick = {
                                haptics.longPress()
                                onExit()
                            },
                            shape = CircleShape,
                            color = Color.Black.copy(alpha = 0.55f),
                            border = BorderStroke(1.dp, Color.White.copy(alpha = 0.25f)),
                            modifier = Modifier
                                .align(Alignment.TopEnd)
                                .padding(8.dp),
                        ) {
                            LlIcon(
                                LlIcons.Close,
                                stringResource(R.string.exit),
                                tint = Color.White,
                                size = 22.dp,
                                // 48 dp across (U4).
                                modifier = Modifier.padding(13.dp),
                            )
                        }
                    }
    }
            }

            // Bottom chrome group: mode toggle, the action disc opening
            // QuickSettingsSheet, and (single-pane) the shortcut bar.
            AnimatedVisibility(
                // The keyboard bar takes this place while the keyboard is open.
                visible = chromeVisible && !keyboardOpen,
                enter = fadeIn(tween(160)) + slideInVertically(tween(240)) { it / 3 },
                exit = fadeOut(tween(240)) + slideOutVertically(tween(240)) { it / 3 },
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .windowInsetsPadding(WindowInsets.navigationBars),
            ) {
                // C5: the chrome's text stops at 1.3x (ChromeText).
                androidx.compose.runtime.CompositionLocalProvider(LocalDensity provides chromeDensity) {
                    Column(
                        modifier = Modifier
                            .onSizeChanged { if (it.height > bottomChromePx) bottomChromePx = it.height }
                            .padding(bottom = 8.dp),
                    ) {
                        // The 16 flat TextButtons collapsed into one floating
                        // action disc opening QuickSettingsSheet. Input mode stays
                        // outside — it is the toggle used mid-gesture — and
                        // doubles as the current-mode readout.
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(horizontal = 8.dp, vertical = 2.dp),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            // A scrimmed chip with the mode's own glyph, matching
                            // the shortcut pill and the HUD: a bare text label
                            // over video reads as a caption, not as the toggle it
                            // is.
                            val modeLabel = when (mode) {
                                InputMode.DirectTouch -> stringResource(R.string.mode_direct_touch)
                                InputMode.Trackpad -> stringResource(R.string.mode_trackpad)
                                InputMode.Pen -> stringResource(R.string.mode_pen)
                            }
                            Row(
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                            // U4: 48 dp to the touch, however small it looks.
                            Row(
                                modifier = Modifier
                                    .minimumInteractiveComponentSize()
                                    .clip(RoundedCornerShape(18.dp))
                                    .background(Color.White.copy(alpha = 0.16f))
                                    .clickable(
                                        role = Role.Button,
                                        // What a tap does, for TalkBack (E6).
                                        onClickLabel = stringResource(R.string.mode_change),
                                    ) { haptics.toggle(); toggleMode() }
                                    .padding(horizontal = 12.dp, vertical = 9.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                LlIcon(
                                    when (mode) {
                                        InputMode.DirectTouch -> LlIcons.Touch
                                        InputMode.Trackpad -> LlIcons.Mouse
                                        InputMode.Pen -> LlIcons.Pen
                                    },
                                    null,
                                    tint = Color.White,
                                    size = 16.dp,
                                )
                                Text(modeLabel, color = Color.White, fontSize = 13.sp)
                            }
                            // F1: always visible while the chrome is, because a
                            // gesture alone is unreliable (phone skins steal
                            // three-finger gestures).
                            Box(
                                modifier = Modifier
                                    .minimumInteractiveComponentSize()
                                    .clip(RoundedCornerShape(18.dp))
                                    .background(Color.White.copy(alpha = 0.16f))
                                    .clickable(role = Role.Button) {
                                        haptics.tap()
                                        keyboardOpen = true
                                        keyboard.open()
                                    }
                                    .padding(horizontal = 12.dp, vertical = 9.dp),
                            ) {
                                LlIcon(
                                    LlIcons.Keyboard,
                                    stringResource(R.string.keyboard_open),
                                    tint = Color.White,
                                    size = 16.dp,
                                )
                            }
                            // One tap to another screen, when there is more than one.
                            ScreenSwitcher(
                                screens = screens,
                                selected = monitorIndex,
                                onPick = { index ->
                                    if (ScreenChoices.isSwitch(screens, monitorIndex, index)) {
                                        haptics.tap()
                                        switchMonitor(index)
                                    }
                                },
                            )
                            }
                            FloatingActionButton(
                                onClick = { haptics.tap(); showQuick = true },
                                shape = CircleShape,
                                containerColor = MaterialTheme.colorScheme.primaryContainer,
                            ) {
                                LlIcon(
                                    LlIcons.MoreVert,
                                    stringResource(R.string.session_menu),
                                    tint = MaterialTheme.colorScheme.onPrimaryContainer,
                                )
                            }
                        }
                        // In dual-pane the shortcut bar lives in the dock beside the
                        // trackpad; here it stays under the stream.
                        if (!dualPane) {
                            ShortcutBar(
                                address = address,
                                port = port,
                                onFailed = { sendTextNotice = it },
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 8.dp, vertical = 6.dp),
                            )
                        }
                    }
    }
            }
        }
        }

        // The dock pane: a native trackpad + shortcut bar on the far side of
        // the hinge (or beside the stream on a tablet). Chrome is hidden in
        // PiP, where the window is thumb-sized and goes to the system.
        if (dualPane && !inPictureInPicture) {
            val dockMod =
                if (vertical) {
                    Modifier.fillMaxHeight().fillMaxWidth(0.5f).align(Alignment.CenterEnd)
                } else {
                    Modifier.fillMaxWidth().height((maxHeight - streamHeight).coerceAtLeast(0.dp))
                        .align(Alignment.BottomCenter)
                }
            ControlDock(
                address = address,
                port = port,
                modifier = dockMod,
                onFailed = { sendTextNotice = it },
            )
        }

        // F1: the IME's target (1 dp, invisible) and the bar above the keyboard.
        AndroidView(
            factory = { KeyboardSinkView(it) },
            update = { keyboard.attach(it) },
            modifier = Modifier.size(1.dp).align(Alignment.TopStart),
        )
        if (keyboardOpen && !inPictureInPicture) {
            KeyboardBar(
                keyboard = keyboard,
                latchVersion = latchVersion,
                haptics = haptics,
                compact = compactBar,
                onHide = {
                    keyboard.close()
                    keyboardOpen = false
                },
                onCompose = { showSendText = true },
                modifier = Modifier.align(Alignment.BottomCenter),
            )
        }

        if (showGamepad && !inPictureInPicture) {
            GamepadOverlay(
                pad = gamepad,
                onChange = ::sendGamepad,
                onClose = {
                    showGamepad = false
                    releaseGamepad()
                },
            )
        }

        // Blackout overlay: last child = topmost. Opaque black covers the
        // (already secure) video surface, the tap detector consumes every
        // gesture so nothing reaches the remote input layer, and a
        // double-tap is the unlock.
        if (blackout && !inPictureInPicture) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color.Black)
                    .pointerInput(Unit) {
                        detectTapGestures(onDoubleTap = { blackout = false })
                    },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    stringResource(R.string.blackout_unlock),
                    color = Color.White.copy(alpha = 0.35f),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
    }
    }

    if (showFiles) {
        RemoteFilesSheet(address = address, controlPort = controlPort, onDismiss = { showFiles = false })
    }

    if (showDeck) {
        DeckSheet(
            address = address,
            controlPort = controlPort,
            computer = dev.pocketdesk.android.HostStore.shownName(context, address),
            onDismiss = { showDeck = false },
        )
    }
    if (showTasks) {
        TasksSheet(address = address, controlPort = controlPort, onDismiss = { showTasks = false })
    }

    if (showSendText) {
        SendTextSheet(
            history = keyboard.sentTexts(),
            onSend = { text, pressEnter ->
                if (keyboard.typeText(text, pressEnter).isFailure) tooLong() else null
            },
            onDismiss = { showSendText = false },
        )
    }

    if (showHistory) {
        ClipboardHistorySheet(
            onDismiss = { showHistory = false },
            onPick = { text ->
                writeLocalClipboard(context, text)
                scope.launch(Dispatchers.IO) {
                    RustCore.sendClipboard(address, controlPort, text)
                }
                showHistory = false
            },
        )
    }

    if (showMonitorPicker) {
        MonitorPickerSheet(
            address = address,
            controlPort = controlPort,
            selected = monitorIndex,
            onDismiss = { showMonitorPicker = false },
            onPick = { index ->
                haptics.tap()
                switchMonitor(index)
            },
            onAddScreen = {
                haptics.tap()
                showMonitorPicker = false
                scope.launch {
                    val answer = withContext(Dispatchers.IO) {
                        RustCore.addScreen(address, controlPort, windowPx.width, windowPx.height)
                    }
                    answer.fold(
                        onSuccess = { json ->
                            when (val a = PhoneScreen.parse(json)) {
                                is PhoneScreen.Answer.Added -> {
                                    addedScreen = true
                                    switchMonitor(a.index, remember = false)
                                }
                                is PhoneScreen.Answer.Refused -> sendTextNotice = a.reason.ifEmpty { screenFailed }
                            }
                        },
                        onFailure = { sendTextNotice = it.message?.let { m -> context.plainErrorText(m) } ?: screenFailed },
                    )
                }
            },
        )
    }

    if (showPairing) {
        PairingSheet(
            address = address,
            controlPort = controlPort,
            message = pairingMessage,
            onDismiss = { showPairing = false },
            onPaired = { serverId ->
                pairedServerId = serverId
                HostStore.savePairedDesktop(context, address, serverId)
                // A computer reached from discovery is saved once paired.
                HostStore.keep(context, HostStore.Host(address, port, controlPort))
                // Paired from the "turned the session down" card: the
                // refused stream comes back without a tap on Retry.
                if (streamStatus is StreamStatus.Down) retryStream()
            },
        )
    }

    if (showDoctor) {
        ConnectionDoctorSheet(
            address = address,
            streamPort = port,
            controlPort = controlPort,
            canWake = false,
            onWake = {},
            // A changed key is fixed only by scanning the code again from
            // Home; the PIN sheet would trust whoever answers.
            onPairAgain = null,
            onDismiss = { showDoctor = false },
        )
    }

    if (showAudio) {
        AudioControlSheet(
            address = address,
            controlPort = controlPort,
            onDismiss = { showAudio = false },
        )
    }

    if (showPicker) {
        WindowPickerSheet(
            address = address,
            controlPort = controlPort,
            selected = cropWindow,
            onDismiss = { showPicker = false },
            onPick = { w, screen ->
                haptics.tap()
                pullWindow(w, screen)
                showPicker = false
            },
            onFullDesktop = {
                haptics.tap()
                cropWindow = null
                cropScreen = null
                scope.launch(Dispatchers.IO) { RustCore.clearWindowCrop() }
                showPicker = false
            },
        )
    }

    if (showQuick) {
        QuickSettingsSheet(
            onDismiss = { showQuick = false },
            mode = mode,
            modes = inputModes(penOffered),
            onSetMode = { mode = it },
            viewOnly = viewOnly,
            onSetViewOnly = ::applyViewOnly,
            qualityPreset = qualityPreset,
            onSetQuality = ::setQuality,
            windowValue = cropWindow?.title?.take(18) ?: stringResource(R.string.value_window_all),
            onPickWindow = {
                showQuick = false
                showPicker = true
            },
            monitorValue =
                if (monitorIndex == -1) {
                    stringResource(R.string.value_monitor_auto)
                } else {
                    stringResource(R.string.value_monitor_indexed, monitorIndex)
                },
            onPickMonitor = {
                showQuick = false
                showMonitorPicker = true
            },
            onGamepad = {
                showQuick = false
                showGamepad = true
            },
            paneMode = paneMode,
            onSetPane = { paneMode = it },
            desktopSound = desktopSound,
            onSetDesktopSound = {
                desktopSound = it
                DesktopAudio.setEnabled(context, it)
            },
            clipboardSync = clipboardSync,
            onSetClipboardSync = { clipboardSync = it },
            onOpenHistory = {
                showQuick = false
                showHistory = true
            },
            onOpenAudio = {
                showQuick = false
                showAudio = true
            },
            onSendText = {
                showQuick = false
                showSendText = true
            },
            onTypeClipboard = {
                showQuick = false
                typePhoneClipboard()
            },
            onOpenFiles = {
                showQuick = false
                showFiles = true
            },
            onOpenTasks = {
                showQuick = false
                showTasks = true
            },
            onOpenDeck = {
                showQuick = false
                showDeck = true
            },
            micOn = micShare.on,
            onSetMic = { if (it) micShare.start() else micShare.stop() },
            privacyGrab = privacyGrab,
            onSetPrivacy = ::applyPrivacy,
            paired = pairedServerId != null,
            onOpenPairing = {
                showQuick = false
                pairingMessage = null
                showPairing = true
            },
            onRing = {
                showQuick = false
                ringComputer()
            },
            onLock = {
                showQuick = false
                confirmLock = true
            },
            onBlackout = {
                showQuick = false
                blackout = true
            },
            onPip = {
                showQuick = false
                (context.findActivity() as? dev.pocketdesk.android.MainActivity)
                    ?.enterSessionPictureInPicture(videoSize.width, videoSize.height)
            },
        )
    }
}

internal tailrec fun Context.findActivity(): Activity? =
    when (this) {
        is Activity -> this
        is ContextWrapper -> baseContext?.findActivity()
        else -> null
    }

/** Below this window height (dp) the keyboard bar uses one row. */
private const val COMPACT_BAR_BELOW_DP = 500
