package dev.pocketdesk.android

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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.ConnectScreen
import dev.pocketdesk.android.ui.HomeScreen
import dev.pocketdesk.android.ui.RemoteScreen
import dev.pocketdesk.android.ui.PairLinkScreen
import dev.pocketdesk.android.ui.QrScanScreen
import dev.pocketdesk.android.ui.SettingsScreen
import dev.pocketdesk.android.ui.theme.PocketdeskTheme

/** Home-family destinations; plain strings so rememberSaveable just works. */
private object Route {
    const val HOME = "home"
    const val FORM = "form"
    const val SETTINGS = "settings"
    const val SCAN = "scan"
}

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
        val host = dev.pocketdesk.android.shortcuts.ShortcutPlan.savedHost(
            intent.getStringExtra(dev.pocketdesk.android.shortcuts.Shortcuts.EXTRA_ADDRESS),
            HostStore.hosts(this),
        )
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
                getString(R.string.shortcut_no_wake, host.address),
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
                app.getString(R.string.wake_sent, host.address)
            } else {
                val error = result.exceptionOrNull()
                app.getString(R.string.wake_failed, error?.message ?: error.toString())
            }
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                android.widget.Toast.makeText(app, message, android.widget.Toast.LENGTH_LONG).show()
            }
        }.start()
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

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        RustCore.start(filesDir)
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
                                    HostStore.save(
                                        ctx,
                                        HostStore.Host(address, link.streamPort, link.controlPort),
                                    )
                                    HostStore.savePairedDesktop(ctx, address, serverId)
                                    link.endpoint?.let { HostStore.saveWanIdentity(ctx, address, it) }
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
                            RemoteScreen(
                                address = session.first,
                                port = session.second,
                                controlPort = session.third,
                                inPictureInPicture = inPictureInPicture.value,
                                onExit = { sessionKey = null },
                            )
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
        if (intent.getBooleanExtra(EXTRA_LOCK_DESKTOP, false)) {
            val address = intent.getStringExtra(
                dev.pocketdesk.android.service.SessionForegroundService.EXTRA_ADDRESS,
            )
            val controlPort = intent.getIntExtra(
                dev.pocketdesk.android.service.SessionForegroundService.EXTRA_CONTROL_PORT,
                HostStore.DEFAULT_CONTROL_PORT,
            )
            if (!address.isNullOrBlank()) {
                Thread {
                    runCatching { RustCore.desktopPrivacy(address, controlPort, "status", lock = true) }
                }.start()
            }
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
private fun parseSession(key: String?): Triple<String, Int, Int>? {
    val parts = key?.split('|') ?: return null
    if (parts.size !in 2..3 || parts[0].isBlank()) return null
    val port = parts[1].toIntOrNull() ?: return null
    val controlPort =
        if (parts.size == 3) {
            parts[2].toIntOrNull() ?: return null
        } else {
            HostStore.DEFAULT_CONTROL_PORT
        }
    return Triple(parts[0], port, controlPort)
}
