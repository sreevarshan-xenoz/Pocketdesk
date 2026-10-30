package dev.pocketdesk.android

import dev.pocketdesk.android.ui.plainErrorText

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.Surface
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.pocketdesk.android.pair.SessionPins
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.ConnectScreen
import dev.pocketdesk.android.ui.HomeScreen
import dev.pocketdesk.android.ui.RemoteInputScreen
import dev.pocketdesk.android.ui.SoundScreen
import dev.pocketdesk.android.ui.RemoteScreen
import dev.pocketdesk.android.ui.PairLinkScreen
import dev.pocketdesk.android.ui.QrScanScreen
import dev.pocketdesk.android.ui.Navigation
import dev.pocketdesk.android.ui.Route
import dev.pocketdesk.android.ui.SettingsScreen
import dev.pocketdesk.android.ui.theme.PocketdeskTheme

class MainActivity : ComponentActivity() {
    companion object {
        /** Set by the notification's Disconnect action (Tier 1 #5). */
        const val EXTRA_DISCONNECT = "dev.pocketdesk.android.DISCONNECT"

        /** Set by the notification's Lock-desktop action (Tier-3 #15). */
        const val EXTRA_LOCK_DESKTOP = "dev.pocketdesk.android.LOCK_DESKTOP"

        private const val STATE_PAIR_LINK = "pair_link"
        /** API level of Android 17, where local network protection starts. */
        private const val ANDROID_17 = 37
        private const val STATE_PAIR_SCANNED = "pair_link_scanned"
    }

    private val disconnectSignal = mutableIntStateOf(0)

    /** Bumped by the notification's Lock: the session asks first (U2). */
    private val lockAskSignal = mutableIntStateOf(0)

    /** A desktop pairing code to act on (pocketdesk://pair?...). */
    private val pairLink = mutableStateOf<String?>(null)

    /** Whether [pairLink] came from this app's scanner rather than a link. */
    private val pairLinkScanned = mutableStateOf(false)

    /** A session a shortcut, the tile or the widget asked for (F11). */
    private val shortcutSession = mutableStateOf<String?>(null)

    /**
     * Whether this launch acted on a shortcut: auto-connecting to the last
     * computer must not also start (a Wake shortcut names a sleeping one).
     */
    private var tookShortcut = false

    /**
     * Act on a shortcut, the tile or the widget (F11): connect to, or wake,
     * a computer already saved. Recents replays the original intent, so a
     * launch from history acts on nothing.
     */
    private fun takeShortcut(intent: Intent) {
        if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        val action = intent.action
        if (action != dev.pocketdesk.android.shortcuts.Shortcuts.ACTION_CONNECT &&
            action != dev.pocketdesk.android.shortcuts.Shortcuts.ACTION_WAKE
        ) {
            return
        }
        setIntent(Intent(intent).setAction(Intent.ACTION_MAIN))
        tookShortcut = true
        // AZ1: one made for a paired computer goes to it, wherever it is now.
        val host = dev.pocketdesk.android.shortcuts.ShortcutPlan.savedHost(
            intent.getStringExtra(dev.pocketdesk.android.shortcuts.Shortcuts.EXTRA_ADDRESS),
            HostStore.hosts(this),
            intent.getStringExtra(dev.pocketdesk.android.shortcuts.Shortcuts.EXTRA_PAIRED_ID),
        ) { HostStore.pairedDesktopId(this, it) }
        if (host == null) {
            android.widget.Toast.makeText(this, R.string.shortcut_not_saved, android.widget.Toast.LENGTH_LONG).show()
            return
        }
        if (action == dev.pocketdesk.android.shortcuts.Shortcuts.ACTION_CONNECT) {
            HostStore.save(this, host)
            dev.pocketdesk.android.shortcuts.Shortcuts.reportConnected(this, host.address)
            shortcutSession.value = "${host.address}|${host.port}|${host.controlPort}"
            return
        }
        if (!dev.pocketdesk.android.wake.Waker.canWake(dev.pocketdesk.android.wake.Waker.planFor(this, host.address))) {
            android.widget.Toast.makeText(
                this,
                getString(R.string.shortcut_no_wake, HostStore.shownName(this, host.address)),
                android.widget.Toast.LENGTH_LONG,
            ).show()
            return
        }
        // The application context, posted to the main thread: a relay may
        // take a while, and must not hold this screen.
        val app = applicationContext
        Thread {
            val result = dev.pocketdesk.android.wake.Waker.wake(app, host.address)
            val message = if (result.isSuccess) {
                app.getString(R.string.wake_sent, HostStore.shownName(app, host.address))
            } else {
                val error = result.exceptionOrNull()
                app.getString(R.string.wake_failed, app.plainErrorText(error?.message))
            }
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                android.widget.Toast.makeText(app, message, android.widget.Toast.LENGTH_LONG).show()
            }
        }.start()
    }

    override fun onResume() {
        super.onResume()
        // Android 17 puts an alarm volume back only while the app shows: a
        // ring that left it raised is put back here (N4 review).
        dev.pocketdesk.android.ring.PhoneRinger.restoreAfterDeath(this)
        // M1: the app in front may start what Android refused from the background.
        val app = applicationContext
        Thread { runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(app) } }.start()
    }

    override fun onStop() {
        super.onStop()
        // The saved computers may have changed; shortcuts follow them.
        dev.pocketdesk.android.shortcuts.Shortcuts.refresh(this)
    }

    private fun pairUriFrom(intent: Intent?): String? =
        intent?.data?.takeIf { it.scheme == "pocketdesk" && it.host == "pair" }?.toString()

    /**
     * The pairing link [intent] carries, taken once: the data is cleared so a
     * recreated activity (theme or locale change, process death) does not
     * offer a pairing the user already finished or dismissed. A launch from
     * Recents replays the original intent, so it carries no new link.
     */
    private fun takePairUri(intent: Intent): String? {
        if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return null
        val uri = pairUriFrom(intent) ?: return null
        setIntent(Intent(intent).setData(null))
        return uri
    }

    /** PiP state (Tier-3 #14), read by the composition to hide chrome. */
    private val inPictureInPicture = mutableStateOf(false)

    private val requestNotificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {
            // Denied is non-fatal: the service still runs, just without a visible notification.
        }

    /** The computer the camera waits for its permission to be shown on (F6). */
    private var cameraTo: HostStore.Host? = null

    private val cameraPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            val host = cameraTo ?: return@registerForActivityResult
            cameraTo = null
            if (granted) dev.pocketdesk.android.cast.CastService.startCamera(this, host.address, host.port)
        }

    /** The computer a cast waits for Android's consent to (T10k). */
    private var castTo: HostStore.Host? = null

    /** M7: that cast is the phone's sound alone. */
    private var castSoundAlone = false

    private val castConsent =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val host = castTo ?: return@registerForActivityResult
            castTo = null
            val data = result.data
            if (result.resultCode == RESULT_OK && data != null) {
                dev.pocketdesk.android.cast.CastService.start(
                    this,
                    host.address,
                    host.port,
                    result.resultCode,
                    data,
                    soundAlone = castSoundAlone,
                )
            }
        }

    /** M7: the computer the phone's sound waits for the microphone permission to go to. */
    private var phoneSoundTo: HostStore.Host? = null

    private val phoneSoundPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            val host = phoneSoundTo ?: return@registerForActivityResult
            phoneSoundTo = null
            if (granted) {
                // Asked only now (M7 review): a second request while the
                // microphone's was up cancelled it.
                maybeRequestNotificationPermission()
                askPhoneSoundConsent(host)
            } else {
                android.widget.Toast.makeText(this, R.string.phone_sound_needs_mic, android.widget.Toast.LENGTH_LONG).show()
            }
        }

    /** M7: Android's consent for the phone's sound alone, said first to be sound only. */
    private fun askPhoneSoundConsent(host: HostStore.Host) {
        // One cast at a time: a second would be dropped after the consent (M7 review).
        if (dev.pocketdesk.android.cast.CastService.castingTo != null) {
            android.widget.Toast.makeText(this, R.string.phone_sound_busy, android.widget.Toast.LENGTH_LONG).show()
            return
        }
        android.widget.Toast.makeText(this, R.string.phone_sound_consent, android.widget.Toast.LENGTH_LONG).show()
        castTo = host
        castSoundAlone = true
        castConsent.launch(
            getSystemService(android.media.projection.MediaProjectionManager::class.java).createScreenCaptureIntent(),
        )
    }

    private val requestLocalNetworkPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (!granted) {
                android.util.Log.w("MainActivity", "local network access denied: LAN connections will time out")
            }
        }

    /** Float the live session over other apps; aspect follows the video. */
    fun enterSessionPictureInPicture(videoWidth: Int, videoHeight: Int) {
        if (videoWidth <= 0 || videoHeight <= 0) {
            enterPictureInPictureMode(
                android.app.PictureInPictureParams.Builder().build(),
            )
            return
        }
        // Android rejects ratios outside [1:2.39, 2.39:1] — clamp both ways.
        val raw = videoWidth.toFloat() / videoHeight.toFloat()
        val ratio = raw.coerceIn(1f / 2.39f, 2.39f)
        enterPictureInPictureMode(
            android.app.PictureInPictureParams.Builder()
                .setAspectRatio(android.util.Rational((ratio * 100).toInt(), 100))
                .build(),
        )
    }

    override fun onPictureInPictureModeChanged(
        isInPictureInPictureMode: Boolean,
        newConfig: android.content.res.Configuration,
    ) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        inPictureInPicture.value = isInPictureInPictureMode
    }

    /**
     * T9c: while a session holds the pointer, a mouse's Back and Forward
     * buttons belong to the desktop; Android also turns them into this
     * phone's Back (fired on the key's release), which would end the session.
     */
    private fun mouseKeySwallowed(keyCode: Int, event: android.view.KeyEvent): Boolean =
        dev.pocketdesk.android.stream.CapturedMouse.swallowsKey(
            keyCode,
            fromMouse = event.isFromSource(android.view.InputDevice.SOURCE_MOUSE),
            captured = dev.pocketdesk.android.stream.CapturedMouse.captured,
        )

    // E4: a game controller's keys and sticks go to the session's gamepad
    // first, while a session takes one. Activity.dispatchKeyEvent is the
    // framework's public method; androidx only marks its own override of it
    // restricted, and a controller's keys must be seen before any view's.
    @android.annotation.SuppressLint("RestrictedApi")
    override fun dispatchKeyEvent(event: android.view.KeyEvent): Boolean =
        dev.pocketdesk.android.stream.ConnectedGamepad.key(event) || super.dispatchKeyEvent(event)

    override fun dispatchGenericMotionEvent(event: android.view.MotionEvent): Boolean =
        dev.pocketdesk.android.stream.ConnectedGamepad.motion(event) || super.dispatchGenericMotionEvent(event)

    override fun onKeyDown(keyCode: Int, event: android.view.KeyEvent): Boolean =
        mouseKeySwallowed(keyCode, event) || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: android.view.KeyEvent): Boolean =
        mouseKeySwallowed(keyCode, event) || super.onKeyUp(keyCode, event)

    // UX2: a debuggable build's font-scale override, for layout checks.
    override fun attachBaseContext(newBase: android.content.Context) {
        super.attachBaseContext(DebugFontScale.wrap(newBase))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        RustCore.start(this)
        // D1: the system build, so a later boot can tell an update.
        dev.pocketdesk.android.doctor.HealthReceiver.noteBuild(this)
        if (savedInstanceState == null) {
            maybeRequestLocalNetworkPermission()
            pairLink.value = takePairUri(intent)
            pairLinkScanned.value = false
            takeShortcut(intent)
            dev.pocketdesk.android.shortcuts.Shortcuts.refresh(this)
        } else {
            pairLink.value = savedInstanceState.getString(STATE_PAIR_LINK)
            pairLinkScanned.value = savedInstanceState.getBoolean(STATE_PAIR_SCANNED)
        }
        setContent {
            PocketdeskTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    var sessionKey by rememberSaveable { mutableStateOf<String?>(null) }
                    // Home-family route (home list / add form / settings),
                    // kept as a name so it survives process save/restore.
                    var route by rememberSaveable { mutableStateOf(Route.HOME) }

                    // M1 review: a switch turned off or the last computer removed
                    // in the app stops the link's service, without a listener
                    // tick (the listener may be unbound); at once on a setting,
                    // else every 10 s while the app is open.
                    val prefsVersion = Prefs.observedVersion()
                    LaunchedEffect(prefsVersion) {
                        val app = applicationContext
                        while (true) {
                            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                                runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(app) }
                            }
                            kotlinx.coroutines.delay(10_000)
                        }
                    }

                    LaunchedEffect(Unit) {
                        if (sessionKey == null &&
                            !tookShortcut &&
                            HostStore.autoConnect(this@MainActivity)
                        ) {
                            HostStore.lastHost(this@MainActivity)?.let {
                                sessionKey = "${it.address}|${it.port}|${it.controlPort}"
                            }
                        }
                    }

                    LaunchedEffect(shortcutSession.value) {
                        shortcutSession.value?.let {
                            maybeRequestNotificationPermission()
                            sessionKey = it
                            shortcutSession.value = null
                        }
                    }

                    LaunchedEffect(disconnectSignal.intValue) {
                        if (disconnectSignal.intValue > 0) {
                            sessionKey = null
                        }
                    }

                    val pairing = pairLink.value
                    if (pairing != null) {
                        Box(Modifier.windowInsetsPadding(WindowInsets.safeDrawing)) {
                            PairLinkScreen(
                                uri = pairing,
                                scanned = pairLinkScanned.value,
                                onPaired = { link, address, serverId ->
                                    val ctx = this@MainActivity
                                    // U3: the name the code carried, until a
                                    // connection says it again.
                                    dev.pocketdesk.android.pair.savePaired(ctx, link, address, serverId)
                                    // Ecosystem phase 4: Home offers its other computers next.
                                    Prefs.markIntroducePending(ctx, address)
                                    pairLink.value = null
                                    maybeRequestNotificationPermission()
                                    sessionKey = "$address|${link.streamPort}|${link.controlPort}"
                                },
                                onBack = { pairLink.value = null },
                            )
                        }
                        return@Surface
                    }

                    when (val session = parseSession(sessionKey)) {
                        // Home-family screens stay clear of the bars; the
                        // session below runs edge-to-edge + immersive on
                        // purpose, so the inset padding is scoped here.
                        null -> Box(Modifier.windowInsetsPadding(WindowInsets.safeDrawing)) {
                            // Back goes up a level, to Home (U2: it left the
                            // app from Settings, Add computer and the scanner).
                            androidx.activity.compose.BackHandler(
                                enabled = Navigation.backTo(route) != null,
                            ) {
                                route = Navigation.backTo(route) ?: Route.HOME
                            }
                            AnimatedContent(
                                targetState = route,
                                transitionSpec = {
                                    (
                                        fadeIn(tween(180)) +
                                            slideInHorizontally(tween(260)) { it / 6 }
                                        ) togetherWith (
                                        fadeOut(tween(120)) +
                                            slideOutHorizontally(tween(260)) { -it / 6 }
                                    )
                                },
                                label = "home-route",
                            ) { current ->
                                when (current) {
                                    Route.FORM -> ConnectScreen(
                                        initial = null,
                                        autoConnect = HostStore.autoConnect(this@MainActivity),
                                        onBack = { route = Route.HOME },
                                        onConnect = { address, port, controlPort, rememberHost ->
                                            HostStore.save(
                                                this@MainActivity,
                                                HostStore.Host(address, port, controlPort),
                                            )
                                            HostStore.setAutoConnect(this@MainActivity, rememberHost)
                                            maybeRequestNotificationPermission()
                                            route = Route.HOME
                                            sessionKey = "$address|$port|$controlPort"
                                        },
                                    )

                                    Route.SETTINGS -> SettingsScreen(
                                        onBack = { route = Route.HOME },
                                    )

                                    Route.SCAN -> QrScanScreen(
                                        onCode = { uri ->
                                            route = Route.HOME
                                            pairLinkScanned.value = true
                                            pairLink.value = uri
                                        },
                                        onBack = { route = Route.HOME },
                                    )

                                    else -> HomeScreen(
                                        onConnect = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            dev.pocketdesk.android.shortcuts.Shortcuts.reportConnected(
                                                this@MainActivity,
                                                host.address,
                                            )
                                            maybeRequestNotificationPermission()
                                            sessionKey =
                                                "${host.address}|${host.port}|${host.controlPort}"
                                        },
                                        onRemote = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            maybeRequestNotificationPermission()
                                            sessionKey =
                                                "${host.address}|${host.port}|${host.controlPort}|$REMOTE_MODE"
                                        },
                                        onSound = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            maybeRequestNotificationPermission()
                                            sessionKey =
                                                "${host.address}|${host.port}|${host.controlPort}|$SOUND_MODE"
                                        },
                                        onCast = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            maybeRequestNotificationPermission()
                                            castTo = host
                                            castSoundAlone = false
                                            castConsent.launch(
                                                getSystemService(android.media.projection.MediaProjectionManager::class.java)
                                                    .createScreenCaptureIntent(),
                                            )
                                        },
                                        onCamera = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            maybeRequestNotificationPermission()
                                            if (checkSelfPermission(android.Manifest.permission.CAMERA) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                                                dev.pocketdesk.android.cast.CastService.startCamera(this@MainActivity, host.address, host.port)
                                            } else {
                                                cameraTo = host
                                                cameraPermission.launch(android.Manifest.permission.CAMERA)
                                            }
                                        },
                                        onPhoneSound = { host ->
                                            HostStore.save(this@MainActivity, host)
                                            if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                                                maybeRequestNotificationPermission()
                                                askPhoneSoundConsent(host)
                                            } else {
                                                phoneSoundTo = host
                                                phoneSoundPermission.launch(android.Manifest.permission.RECORD_AUDIO)
                                            }
                                        },
                                        onConnectFound = { host ->
                                            // Not saved until it pairs: an mDNS
                                            // announcement proves nothing.
                                            maybeRequestNotificationPermission()
                                            sessionKey =
                                                "${host.address}|${host.port}|${host.controlPort}"
                                        },
                                        onAdd = { route = Route.FORM },
                                        onScan = { route = Route.SCAN },
                                        onOpenSettings = { route = Route.SETTINGS },
                                    )
                                }
                            }
                        }

                        // Keyed by the session: a shortcut can name another
                        // computer while one is on screen, and the old session
                        // (decoder, service, input) must end, not be reused
                        // with half its parts pointed at the new computer.
                        else -> key(sessionKey) {
                            // Its pin goes as it ends with no card saved
                            // there (BC3): a found computer that never paired.
                            DisposableEffect(Unit) {
                                val app = applicationContext
                                val address = session.address
                                SessionPins.current = address
                                onDispose {
                                    if (SessionPins.current == address) SessionPins.current = null
                                    SessionPins.afterSession(app, address)
                                }
                            }
                            if (session.remote) {
                                // The phone as a remote (T10i): no picture.
                                Box(Modifier.windowInsetsPadding(WindowInsets.safeDrawing)) {
                                    RemoteInputScreen(
                                        address = session.address,
                                        port = session.port,
                                        controlPort = session.controlPort,
                                        askLock = lockAskSignal.intValue,
                                        onExit = { sessionKey = null },
                                    )
                                }
                            } else if (session.sound) {
                                // The phone as the computer's speaker and mic (N5).
                                Box(Modifier.windowInsetsPadding(WindowInsets.safeDrawing)) {
                                    SoundScreen(
                                        address = session.address,
                                        port = session.port,
                                        controlPort = session.controlPort,
                                        askLock = lockAskSignal.intValue,
                                        onExit = { sessionKey = null },
                                    )
                                }
                            } else {
                                RemoteScreen(
                                    address = session.address,
                                    port = session.port,
                                    controlPort = session.controlPort,
                                    inPictureInPicture = inPictureInPicture.value,
                                    askLock = lockAskSignal.intValue,
                                    onExit = { sessionKey = null },
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putString(STATE_PAIR_LINK, pairLink.value)
        outState.putBoolean(STATE_PAIR_SCANNED, pairLinkScanned.value)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        takePairUri(intent)?.let {
            pairLinkScanned.value = false
            pairLink.value = it
        }
        takeShortcut(intent)
        if (intent.getBooleanExtra(EXTRA_DISCONNECT, false)) {
            Thread { runCatching { RustCore.stopStreaming() } }.start()
            disconnectSignal.intValue += 1
        }
        // The session (or the remote) on screen asks first and says what
        // happened (U2); it had locked at one tap from the shade.
        if (intent.getBooleanExtra(EXTRA_LOCK_DESKTOP, false)) {
            lockAskSignal.intValue += 1
        }
    }

    /**
     * Android 17 blocks the local network (every LAN connection, and finding
     * computers on it) for apps that target it, until the user allows it.
     * Asked at launch, since nothing works on a LAN without it.
     */
    private fun maybeRequestLocalNetworkPermission() {
        if (Build.VERSION.SDK_INT >= ANDROID_17 &&
            checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            requestLocalNetworkPermission.launch(Manifest.permission.ACCESS_LOCAL_NETWORK)
        }
    }

    private fun maybeRequestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            requestNotificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}

/** Session state travels as "address|streamingPort|controlPort" so it survives process save/restore. */
/** The session on screen: a computer, and whether the phone is its remote (T10i). */
private data class SessionTarget(
    val address: String,
    val port: Int,
    val controlPort: Int,
    val remote: Boolean,
    /** The phone as the computer's speaker and mic (N5). */
    val sound: Boolean = false,
)

/** The session key's mark for the phone as a remote (T10i). */
private const val REMOTE_MODE = "remote"

/** The session key's mark for a sound session (N5). */
private const val SOUND_MODE = "sound"

private fun parseSession(key: String?): SessionTarget? {
    val parts = key?.split('|') ?: return null
    if (parts.size !in 2..4 || parts[0].isBlank()) return null
    val port = parts[1].toIntOrNull() ?: return null
    val controlPort =
        if (parts.size >= 3) {
            parts[2].toIntOrNull() ?: return null
        } else {
            HostStore.DEFAULT_CONTROL_PORT
        }
    val remote = parts.size == 4 && parts[3] == REMOTE_MODE
    val sound = parts.size == 4 && parts[3] == SOUND_MODE
    if (parts.size == 4 && !remote && !sound) return null
    return SessionTarget(parts[0], port, controlPort, remote, sound)
}
