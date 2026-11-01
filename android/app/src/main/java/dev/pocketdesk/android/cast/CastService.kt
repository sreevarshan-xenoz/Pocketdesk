package dev.pocketdesk.android.cast

import dev.pocketdesk.android.ui.plainErrorText

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.drawable.Icon
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.RemoteSession
import dev.pocketdesk.android.ui.RemoteSession.State
import dev.pocketdesk.android.windows.ShizukuBridge
import dev.pocketdesk.android.windows.WindowCast
import dev.pocketdesk.android.windows.WindowInput
import dev.pocketdesk.android.windows.WindowRule

/**
 * The phone's screen on the computer (T10k). Started with the owner's
 * consent (Android's "Start recording or casting?"), it opens a cast session
 * to the computer (the LAN, else the internet road), waits for the computer
 * to admit it, then encodes the screen ([CastEncoder]) from a virtual display
 * of the projection, at the phone's shape, turning with it. It ends when the
 * computer ends the session, from its notification, or when Android stops
 * the projection (the status bar's stop).
 */
class CastService : Service() {
    companion object {
        const val ACTION_START = "dev.pocketdesk.android.action.START_CAST"
        const val ACTION_STOP = "dev.pocketdesk.android.action.STOP_CAST"
        const val EXTRA_ADDRESS = "address"
        const val EXTRA_PORT = "port"
        const val EXTRA_RESULT_CODE = "resultCode"
        const val EXTRA_RESULT_DATA = "resultData"
        const val EXTRA_CAMERA = "camera"
        const val EXTRA_SOUND_ALONE = "soundAlone"
        const val EXTRA_WEBCAM = "webcam"
        const val EXTRA_FRONT = "front"
        const val EXTRA_HEIGHT = "height"
        const val EXTRA_FPS = "fps"

        /** Phone apps tier 2: open one app in a window of its own. */
        const val ACTION_OPEN_WINDOW = "dev.pocketdesk.android.action.OPEN_WINDOW"
        const val EXTRA_CONTROL_PORT = "controlPort"
        const val EXTRA_APP = "app"
        const val EXTRA_WINDOW = "window"
        const val EXTRA_WINDOW_WIDTH = "windowWidth"
        const val EXTRA_WINDOW_HEIGHT = "windowHeight"
        const val EXTRA_WINDOW_DPI = "windowDpi"

        private const val TAG = "CastService"
        private const val CHANNEL_ID = "pocketdesk-cast"

        /**
         * The computer's camera in use (F6 part 2): a channel of its own, of
         * default importance, so its notification stays plain to see; no
         * sound or vibration (PR #604 review N4).
         */
        private const val WEBCAM_CHANNEL_ID = "pocketdesk-webcam"
        private const val NOTIFICATION_ID = 7
        private const val FAILED_NOTIFICATION_ID = 8
        private const val POLL_MS = 500L

        /** A session of app windows alone ends this long after its last window closed. */
        private const val EMPTY_GRACE_MS = 3_000L

        /** The owner may be asked; the computer gives them 30 s. */
        private const val WAIT_MS = 45_000L

        /** The computer the screen is shown on now, for the UI; null when none. */
        @Volatile
        var castingTo: String? = null
            private set

        /**
         * The computer the phone's screen (not its camera, not its sound
         * alone) is shown on now, null when none: what an open that wants
         * the screen reuses instead of asking for Android's consent again
         * (Apps part 1).
         */
        @Volatile
        var screenTo: String? = null
            private set

        /**
         * The computer whose screen share is paused, null when none
         * (Hands-free): the owner's consent and the projection are kept for
         * [CastPauseRule.PAUSE_MS], with nothing captured, so that computer's
         * next ask resumes with no new question. Never for another computer.
         */
        @Volatile
        var pausedFor: String? = null
            private set

        @Volatile
        private var current: CastService? = null

        /** The ids of the app windows open or asked for now (Phone apps tier 2), for the limit. */
        val windowIds: Set<Int>
            get() = current?.let { service ->
                service.pendingWindows.map { it.ask.id }.toSet() + (service.windowCast?.ids ?: emptySet())
            } ?: emptySet()

        /**
         * Open [pkg] in window [ask] for the computer [own] (Phone apps tier
         * 2). It rides the cast to that computer if one is up, else starts a
         * session of app windows alone: no consent is asked, as nothing of the
         * phone's screen is captured. False when Android would not start the
         * service (the phone is in the background and not allowed to).
         */
        fun openWindow(context: Context, own: HostStore.Host, pkg: String, ask: WindowRule.Ask): Boolean =
            runCatching {
                context.startForegroundService(
                    Intent(context, CastService::class.java)
                        .setAction(ACTION_OPEN_WINDOW)
                        .putExtra(EXTRA_ADDRESS, own.address)
                        .putExtra(EXTRA_PORT, own.port)
                        .putExtra(EXTRA_CONTROL_PORT, own.controlPort)
                        .putExtra(EXTRA_APP, pkg)
                        .putExtra(EXTRA_WINDOW, ask.id)
                        .putExtra(EXTRA_WINDOW_WIDTH, ask.width)
                        .putExtra(EXTRA_WINDOW_HEIGHT, ask.height)
                        .putExtra(EXTRA_WINDOW_DPI, ask.dpi),
                )
            }.onFailure { Log.w(TAG, "the window service did not start", it) }.isSuccess

        /** The computer's input to app windows (from [dev.pocketdesk.android.control.ControlRunner]). */
        fun windowInput(context: Context, events: List<WindowInput.For>) {
            // Checked again here, not only each half second: the switch just went off.
            if (!Prefs.separateWindows(context)) return
            val cast = current?.windowCast ?: return
            events.forEach { cast.input(it.window, it.event) }
        }

        /**
         * Resume the paused screen share to [address] (Hands-free), when
         * [CastPauseRule.mayResume] says so: true when a resume was begun
         * (the cast is up when [screenTo] names the computer), false when
         * nothing is paused for it or it may not.
         */
        fun resume(context: Context, address: String, port: Int): Boolean {
            val service = current ?: return false
            val own = dev.pocketdesk.android.notify.PhoneLink.target(context)?.address == address
            return service.resumeFor(address, port, Prefs.appsFromComputer(context), Prefs.autoAcceptScreen(context), own)
        }

        /** Whether the camera is the computer's camera now (F6 part 2). */
        @Volatile
        var webcamRunning: Boolean = false
            private set

        /**
         * Stop the camera the computer uses, if it is in use (F6 part 2):
         * "Let the computer use my camera" turned off closes it at once
         * (PR #604 review N4).
         */
        fun stopWebcam(context: Context) {
            if (!webcamRunning) return
            runCatching {
                context.startService(Intent(context, CastService::class.java).setAction(ACTION_STOP))
            }
        }

        /**
         * Show the phone's back camera on [address] (F6). Only with the
         * camera permission: a camera service started without it is
         * refused by Android 14 before it can say why (F6 review).
         */
        fun startCamera(context: Context, address: String, port: Int) {
            if (context.checkSelfPermission(android.Manifest.permission.CAMERA) != android.content.pm.PackageManager.PERMISSION_GRANTED) return
            context.startForegroundService(
                Intent(context, CastService::class.java)
                    .setAction(ACTION_START)
                    .putExtra(EXTRA_ADDRESS, address)
                    .putExtra(EXTRA_PORT, port)
                    .putExtra(EXTRA_CAMERA, true),
            )
        }

        /**
         * Be the camera of the computer at [address] (F6 part 2), as it
         * asked: only from [WebcamActivity], opened by a tap on the ask, and
         * only with the camera permission. The camera is open only while
         * this service's notification shows.
         */
        fun startWebcam(context: Context, address: String, port: Int, ask: WebcamAsk.Ask) {
            if (context.checkSelfPermission(android.Manifest.permission.CAMERA) != android.content.pm.PackageManager.PERMISSION_GRANTED) return
            context.startForegroundService(
                Intent(context, CastService::class.java)
                    .setAction(ACTION_START)
                    .putExtra(EXTRA_ADDRESS, address)
                    .putExtra(EXTRA_PORT, port)
                    .putExtra(EXTRA_CAMERA, true)
                    .putExtra(EXTRA_WEBCAM, true)
                    .putExtra(EXTRA_FRONT, ask.front)
                    .putExtra(EXTRA_HEIGHT, ask.height)
                    .putExtra(EXTRA_FPS, ask.fps),
            )
        }

        /**
         * Start a cast to [address] with the consent Android returned; with
         * [soundAlone], the phone's sound and no picture (M7).
         */
        fun start(context: Context, address: String, port: Int, resultCode: Int, data: Intent, soundAlone: Boolean = false) {
            context.startForegroundService(
                Intent(context, CastService::class.java)
                    .setAction(ACTION_START)
                    .putExtra(EXTRA_ADDRESS, address)
                    .putExtra(EXTRA_PORT, port)
                    .putExtra(EXTRA_RESULT_CODE, resultCode)
                    .putExtra(EXTRA_RESULT_DATA, data)
                    .putExtra(EXTRA_SOUND_ALONE, soundAlone),
            )
        }
    }

    private val main = Handler(Looper.getMainLooper())
    private var address = ""

    /** The computer as the owner named it, else as it calls itself (E6: not its address). */
    private val shown: String get() = dev.pocketdesk.android.HostStore.shownName(this, address)
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var encoder: CastEncoder? = null

    /** F6: the back camera instead of the screen, and its feed once shown. */
    private var camera = false
    private var feed: CameraFeed? = null

    /** F6 part 2: the camera as the computer's camera, and what it asked for. */
    private var webcam = false
    private var webcamAsk = WebcamAsk.Ask(front = false, height = 1080, fps = 0)

    /** Phone apps tier 2: app windows only, no screen of the phone's. */
    @Volatile
    private var windowsOnly = false

    /** The app windows of this cast, made when the first is asked for. */
    @Volatile
    private var windowCast: WindowCast? = null

    /** An app window asked for before the cast was admitted. */
    private class PendingWindow(val ask: WindowRule.Ask, val pkg: String, val controlPort: Int)

    private val pendingWindows = java.util.concurrent.CopyOnWriteArrayList<PendingWindow>()

    /** The cast was admitted: app windows may be made. */
    @Volatile
    private var windowsLive = false

    /** M7: the phone's sound alone, no picture. */
    private var soundAlone = false
    private var started = false

    @Volatile
    private var finished = false

    /** UX2 (review N10): it reached the computer, so its card is stamped "Used" as it ends too. */
    @Volatile private var reached = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        current = this
    }

    /** Hands-free: the screen share is paused, its projection and display kept. */
    @Volatile
    private var paused = false
    private var pausedAt = 0L

    /** A resumed session not yet admitted: its failure pauses again instead of ending the cast. */
    @Volatile
    private var resuming = false

    /**
     * While paused, every [CastPauseRule.WATCH_MS]: the share is given back at
     * once when the apps switch or the opt-in turns off, or the computer is no
     * longer the phone's own (unpaired), not only when it is next asked for.
     */
    private val pauseWatch = object : Runnable {
        override fun run() {
            if (!paused || finished) return
            val own = dev.pocketdesk.android.notify.PhoneLink.target(this@CastService)?.address == address
            if (!CastPauseRule.kept(Prefs.appsFromComputer(this@CastService), Prefs.autoAcceptScreen(this@CastService), own)) {
                Log.i(TAG, "A paused screen share is no longer allowed: the capture is given back")
                finish()
                return
            }
            main.postDelayed(this, CastPauseRule.WATCH_MS)
        }
    }

    /** Ends a pause that ran out. */
    private val pauseExpiry = Runnable {
        if (paused && !finished) {
            Log.i(TAG, "The paused screen share ran out: the capture is given back")
            finish()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            finish()
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_OPEN_WINDOW) {
            onOpenWindow(intent)
            return START_NOT_STICKY
        }
        // A new consent while one is kept paused (for another computer, or the
        // owner asked again): the old projection is given back first, and the
        // new one starts as any cast does.
        if (intent?.action == ACTION_START && paused && !finished) dropPause()
        // One cast at a time: a second start while one runs is ignored.
        if (intent?.action != ACTION_START || started || finished) {
            if (!started) stopSelf()
            return START_NOT_STICKY
        }
        address = intent.getStringExtra(EXTRA_ADDRESS).orEmpty()
        val port = intent.getIntExtra(EXTRA_PORT, 0)
        // F6: the back camera, not the screen: no projection, a camera service.
        if (intent.getBooleanExtra(EXTRA_CAMERA, false)) {
            camera = true
            webcam = intent.getBooleanExtra(EXTRA_WEBCAM, false)
            if (webcam) {
                webcamAsk = WebcamAsk.Ask(
                    intent.getBooleanExtra(EXTRA_FRONT, false),
                    intent.getIntExtra(EXTRA_HEIGHT, 720),
                    intent.getIntExtra(EXTRA_FPS, 30),
                )
            }
            started = true
            webcamRunning = webcam
            createChannel()
            val notification = notification(getString(R.string.cast_connecting, shown))
            // The permission taken back since it was asked: Android refuses
            // the camera service, so it ends here, saying why (F6 review).
            val foreground = runCatching {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA)
                } else {
                    startForeground(NOTIFICATION_ID, notification)
                }
            }
            if (foreground.isFailure || address.isEmpty() ||
                checkSelfPermission(android.Manifest.permission.CAMERA) != android.content.pm.PackageManager.PERMISSION_GRANTED
            ) {
                fail(getString(R.string.cast_camera_denied))
                return START_NOT_STICKY
            }
            castingTo = address
            Thread({ run(port) }, "cast-session").start()
            return START_NOT_STICKY
        }
        soundAlone = intent.getBooleanExtra(EXTRA_SOUND_ALONE, false)
        val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, Activity.RESULT_CANCELED)
        val data =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(EXTRA_RESULT_DATA, Intent::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(EXTRA_RESULT_DATA)
            }
        createChannel()
        // The foreground service's type must be in place before the
        // projection is taken (Android 14).
        val notification = notification(getString(R.string.cast_connecting, shown))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        val manager = getSystemService(MediaProjectionManager::class.java)
        val taken = data?.let { runCatching { manager.getMediaProjection(resultCode, it) }.getOrNull() }
        if (address.isEmpty() || taken == null) {
            // A projection taken for no computer is given back at once.
            runCatching { taken?.stop() }
            fail(getString(R.string.cast_no_consent))
            return START_NOT_STICKY
        }
        projection = taken
        started = true
        taken.registerCallback(
            object : MediaProjection.Callback() {
                // Android ended it (the status bar's stop, or a lock): a
                // paused cast ends with it. A projection given back on
                // purpose (a dropped pause) is no longer this service's.
                override fun onStop() {
                    if (projection === taken) finish()
                }
            },
            main,
        )
        castingTo = address
        screenTo = address.takeUnless { soundAlone }
        Thread({ run(port) }, "cast-session").start()
        return START_NOT_STICKY
    }

    /** What the phone's apps play, with its screen (K5). */
    @Volatile
    private var sound: CastSound? = null

    /**
     * Start the cast's sound when everything says it may go (K5); asked for
     * alone (M7), the setting for the screen's sound does not apply.
     */
    private fun startSound(castState: String) {
        val taken = projection ?: return
        val granted =
            checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
                android.content.pm.PackageManager.PERMISSION_GRANTED
        if (!CastSoundRule.capture(
                soundAlone || Prefs.castSound(this),
                granted,
                CastSoundRule.computerTakes(castState),
                camera,
                Build.VERSION.SDK_INT,
            )
        ) {
            return
        }
        if (Build.VERSION.SDK_INT >= CastSoundRule.MIN_SDK) {
            sound = CastSound(taken).takeIf { it.start() }
            // A cast that ended meanwhile stops it (K5 review).
            if (finished) {
                sound?.stop()
                sound = null
            }
        }
    }

    /** Control from the computer while the screen is shown (C8). */
    @Volatile
    private var control: dev.pocketdesk.android.control.ControlRunner? = null

    /** The session: connect, wait to be admitted, show, and watch it. */
    private fun run(port: Int) {
        val connected =
            RustCore.connectCast(address, port, soundAlone, webcam, windowsOnly).recoverCatching { lan ->
                if (finished) throw lan
                val identity = HostStore.wanIdentity(this, address) ?: throw lan
                RustCore.connectCastWan(address, identity, soundAlone, webcam, windowsOnly).getOrThrow()
            }
        if (finished) return
        connected.exceptionOrNull()?.let {
            failOrPause(failed(plainErrorText(it.message)))
            return
        }
        update(getString(R.string.cast_waiting, shown))
        var state = State.Waiting
        val since = SystemClock.elapsedRealtime()
        var showing = false
        while (!finished) {
            val closed = RustCore.streamClosedReason
            // Only the cast's own session: another the person opened since
            // ends the cast, quietly, and is left alone.
            val castState = RustCore.castState()
            val cast = RemoteSession.parse(castState)
            val replaced = cast == "none" && state != State.Connecting
            state = RemoteSession.next(state, cast, closed.takeUnless { replaced })
            if (state == State.Waiting && SystemClock.elapsedRealtime() - since > WAIT_MS) {
                failOrPause(failed(getString(R.string.cast_no_answer)))
                return
            }
            // Phone apps tier 2: app windows turned off, Shizuku gone, or the
            // computer no longer the phone's own: they are given back.
            watchWindows()
            // M7: the sound alone ended by itself (the capture or the
            // link failed): nothing is left to do (M7 review: it stayed up
            // silent until Stop).
            if (soundAlone && showing && sound?.alive != true) {
                return fail(failed(getString(R.string.phone_sound_no_capture)))
            }
            when (state) {
                State.TooOld ->
                    return fail(getString(if (soundAlone) R.string.phone_sound_too_old else R.string.cast_too_old, shown))
                State.Down -> {
                    if (replaced) return finish()
                    // Hands-free: the computer left a screen it was showing:
                    // keep the owner's consent for a while instead of ending.
                    if (CastPauseRule.pauses(
                            screen = !camera && !soundAlone && !windowsOnly,
                            shown = showing,
                            appsOn = Prefs.appsFromComputer(this),
                            optIn = Prefs.autoAcceptScreen(this),
                            ownComputer = dev.pocketdesk.android.notify.PhoneLink.target(this)?.address == address,
                            closed = closed,
                        )
                    ) {
                        return pause()
                    }
                    if (!showing || closed != null && closed != "session ended") {
                        return failOrPause(failed(reasonText(closed)))
                    }
                    return finish()
                }
                State.Ready -> {
                    // UX2 (review N10): a cast, camera or phone-sound session
                    // stamps its card "Used just now" too.
                    if (!reached) {
                        reached = true
                        HostStore.markUsed(this, address)
                    }
                    // App windows may ride any cast of the phone's screen or be
                    // the whole session (Phone apps tier 2).
                    if (!soundAlone && !camera && !windowsLive) {
                        windowsLive = true
                        drainPendingWindows()
                    }
                    if (!showing && windowsOnly) {
                        showing = true
                        resuming = false
                        update(getString(R.string.cast_showing_windows, shown))
                        // The computer's touch and typing in the windows.
                        control = dev.pocketdesk.android.control.ControlRunner(this) { }.also { it.start() }
                    } else if (!showing && soundAlone) {
                        showing = true
                        // M7: its sound only: no picture, no control.
                        startSound(castState)
                        if (sound == null) {
                            return fail(failed(getString(R.string.phone_sound_no_capture)))
                        }
                        update(getString(R.string.phone_sound_playing, shown))
                    } else if (!showing) {
                        showing = true
                        resuming = false
                        main.post(::startPicture)
                        startSound(castState)
                        update(
                            getString(
                                when {
                                    webcam -> R.string.cast_showing_webcam
                                    camera -> R.string.cast_showing_camera
                                    else -> R.string.cast_showing
                                },
                                shown,
                            ),
                        )
                        // C8: the computer may control the phone while its screen is shown.
                        if (!camera) control = dev.pocketdesk.android.control.ControlRunner(this) { on ->
                            if (!finished) {
                                update(getString(if (on) R.string.cast_showing_controlled else R.string.cast_showing, shown))
                            }
                        }.also { it.start() }
                    }
                }
                else -> {}
            }
            Thread.sleep(POLL_MS)
        }
    }

    /**
     * Phone apps tier 2: an ask to open an app in a window. A fresh service
     * becomes a session of app windows alone; a running screen cast to the
     * same computer carries the window too; anything else is refused.
     */
    private fun onOpenWindow(intent: Intent) {
        val to = intent.getStringExtra(EXTRA_ADDRESS).orEmpty()
        val port = intent.getIntExtra(EXTRA_PORT, 0)
        val controlPort = intent.getIntExtra(EXTRA_CONTROL_PORT, 0)
        val pkg = intent.getStringExtra(EXTRA_APP).orEmpty()
        val ask = WindowRule.Ask(
            intent.getIntExtra(EXTRA_WINDOW, 0),
            intent.getIntExtra(EXTRA_WINDOW_WIDTH, 0),
            intent.getIntExtra(EXTRA_WINDOW_HEIGHT, 0),
            intent.getIntExtra(EXTRA_WINDOW_DPI, 0),
        )
        val fresh = !started && !finished
        // A service started as a foreground one must say so at once, whatever follows.
        createChannel()
        val notification = notification(getString(R.string.cast_connecting, HostStore.shownName(this, to)))
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        }.onFailure { Log.w(TAG, "the window service could not go foreground", it) }
        val usable = to.isNotEmpty() && pkg.isNotEmpty() && ask.sound && !finished && !paused && !camera &&
            !soundAlone && (fresh || address == to)
        if (!usable) {
            Log.i(TAG, "an app window was not made: no usable session")
            Thread { dev.pocketdesk.android.control.PhoneActions.report(to, controlPort, "open", pkg, dev.pocketdesk.android.control.AppOpenRule.WINDOWS_UNAVAILABLE, window = ask.id) }.start()
            if (fresh) {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
            return
        }
        pendingWindows.add(PendingWindow(ask, pkg, controlPort))
        if (fresh) {
            address = to
            windowsOnly = true
            started = true
            castingTo = to
            Thread({ run(port) }, "cast-session").start()
        } else if (windowsLive) {
            drainPendingWindows()
        }
    }

    /** The app windows asked for before the cast was admitted are made now. */
    private fun drainPendingWindows() {
        if (finished) return
        val asked = pendingWindows.toList()
        pendingWindows.clear()
        if (asked.isEmpty()) return
        val cast = windowCast ?: WindowCast(this) { onWindowsEmpty() }.also { windowCast = it }
        for (one in asked) {
            val to = address
            cast.open(one.ask, one.pkg) { word ->
                Log.i(TAG, "an app window: $word")
                dev.pocketdesk.android.control.PhoneActions.report(to, one.controlPort, "open", one.pkg, word, window = one.ask.id)
            }
        }
    }

    /** The last app window closed: a session of windows alone ends after a moment. */
    private fun onWindowsEmpty() {
        if (!windowsOnly) return
        main.postDelayed({
            if (windowsOnly && !finished && pendingWindows.isEmpty() && windowCast?.isEmpty != false) finish()
        }, EMPTY_GRACE_MS)
    }

    /**
     * Every half second while the cast runs: app windows are given back at
     * once when the switch is turned off, Shizuku is gone, "Let the computer
     * open my apps" is off, or the computer is no longer the phone's own.
     */
    private fun watchWindows() {
        val cast = windowCast ?: return
        val mode = WindowRule.mode(
            Build.VERSION.SDK_INT,
            ShizukuBridge.running(),
            ShizukuBridge.permitted(),
            Prefs.separateWindows(this),
        )
        val own = dev.pocketdesk.android.notify.PhoneLink.target(this)?.address == address
        if (mode == WindowRule.Mode.ON && Prefs.appsFromComputer(this) && own) return
        Log.i(TAG, "App windows are no longer allowed: they are given back")
        windowCast = null
        cast.closeAll()
        if (windowsOnly) finish()
    }

    /** Why it could not start, as a sentence naming the computer. */
    private fun failed(reason: String): String =
        getString(if (soundAlone) R.string.phone_sound_failed else R.string.cast_failed, shown, reason)

    private fun reasonText(closed: String?): String =
        when {
            closed == null -> getString(R.string.cast_lost)
            closed.contains("phone sound not allowed") -> getString(R.string.phone_sound_not_allowed)
            closed.contains("phone sound not played here") -> getString(R.string.phone_sound_not_played)
            closed.contains("phone camera not allowed") -> getString(R.string.webcam_not_allowed)
            closed.contains("phone camera not offered here") -> getString(R.string.webcam_not_offered)
            closed.contains("another phone is the camera now") -> getString(R.string.webcam_replaced)
            closed.contains("phone camera failed") -> getString(R.string.webcam_failed)
            closed.contains("phone screen not allowed") -> getString(R.string.cast_not_allowed)
            closed.contains("phone screen not shown here") -> getString(R.string.cast_not_shown_here)
            // The computer's own ends, as the session's card says them (S2 review).
            closed.contains("not paired") || closed.contains("permissions changed") ->
                getString(dev.pocketdesk.android.ui.humanizeError(closed))
            else -> closed
        }

    /** The screen's size now, as the display draws it. */
    private fun screen(): Triple<Int, Int, Int> {
        val shown = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        shown.getRealMetrics(metrics)
        return Triple(metrics.widthPixels, metrics.heightPixels, metrics.densityDpi)
    }

    private fun startPicture() {
        if (camera) return startCamera()
        val projection = projection ?: return
        if (finished || encoder != null) return
        val (w, h, dpi) = screen()
        val (width, height) = CastPlan.encodeSize(w, h)
        val made = runCatching { CastEncoder(width, height, ::encoderEnded) }
        val started = made.getOrElse {
            Log.w(TAG, "No encoder for the cast", it)
            return fail(getString(R.string.cast_failed, shown, plainErrorText(it.message)))
        }
        encoder = started
        // Resumed after a pause: the display the first consent made is still
        // here, so it is drawn into the new encoder (a second display from
        // one projection is refused on Android 14).
        display?.let { kept ->
            kept.resize(width, height, dpi)
            kept.surface = started.surface
            return
        }
        display =
            runCatching {
                projection.createVirtualDisplay(
                    "Pocketdesk cast",
                    width,
                    height,
                    dpi,
                    DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                    started.surface,
                    null,
                    null,
                )
            }.getOrElse {
                Log.w(TAG, "No virtual display for the cast", it)
                return fail(getString(R.string.cast_failed, shown, plainErrorText(it.message)))
            }
    }

    /** F6: the back camera into a new encoder at the camera's size. */
    private fun startCamera() {
        if (finished || encoder != null) return
        val made = CameraFeed(this, webcamAsk.front, webcamAsk.height, webcamAsk.fps) {
            fail(getString(R.string.cast_camera_lost))
        }
        val (width, height) = made.size() ?: run {
            made.close()
            return fail(getString(R.string.cast_no_camera))
        }
        val coding = runCatching { CastEncoder(width, height, ::encoderEnded) }.getOrElse {
            made.close()
            Log.w(TAG, "No encoder for the camera", it)
            return fail(getString(R.string.cast_failed, shown, plainErrorText(it.message)))
        }
        encoder = coding
        feed = made
        if (!made.open(coding.surface)) fail(getString(R.string.cast_no_camera))
    }

    private fun encoderEnded() = finish()

    /** A turned phone: a new encoder at the new shape, the display drawn into it. */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // The camera's frames keep their shape whichever way the phone turns.
        if (camera) return
        val old = encoder ?: return
        val shown = display ?: return
        val (w, h, dpi) = screen()
        val (width, height) = CastPlan.encodeSize(w, h)
        if (width == old.width && height == old.height) return
        val next = runCatching { CastEncoder(width, height, ::encoderEnded) }.getOrElse {
            Log.w(TAG, "No encoder for the turned screen", it)
            return
        }
        encoder = next
        shown.resize(width, height, dpi)
        shown.surface = next.surface
        Thread { old.release() }.start()
    }

    /**
     * Hands-free: the computer left a screen it was showing. The encoder, the
     * computer's session, the phone's sound and control stop and the virtual
     * display is detached from its surface, so nothing is captured; the
     * projection and the display stay, and the notification stays up (with
     * Stop), for [CastPauseRule.PAUSE_MS] at most. Called from the session's
     * own thread.
     */
    private fun pause() {
        if (finished || paused) return
        Log.i(TAG, "The computer left: the screen share is paused for up to ${CastPauseRule.PAUSE_MS / 60_000L} minutes")
        castingTo = null
        screenTo = null
        val capture = sound
        sound = null
        val runner = control
        control = null
        main.post {
            if (finished) return@post
            val coding = encoder
            encoder = null
            runner?.stop()
            // Detached: the display draws into nothing, the projection stays.
            runCatching { display?.surface = null }
            paused = true
            pausedAt = SystemClock.elapsedRealtime()
            showPaused()
            main.removeCallbacks(pauseExpiry)
            main.removeCallbacks(pauseWatch)
            main.postDelayed(pauseExpiry, CastPauseRule.PAUSE_MS)
            main.postDelayed(pauseWatch, CastPauseRule.WATCH_MS)
            Thread {
                capture?.stop()
                coding?.release()
                runCatching { RustCore.stopCast() }
                // Resumable only once the old session is gone.
                if (!finished && paused) pausedFor = address
            }.start()
        }
    }

    /** The notification of a paused share: it says so, and when it ends, with Stop. */
    private fun showPaused() {
        val minutes = CastPauseRule.minutesLeft(pausedAt, SystemClock.elapsedRealtime())
        update(getString(R.string.cast_paused, shown, minutes))
    }

    /**
     * Resume a paused screen share to [to] (Hands-free), from any thread, if
     * the rule says so. The same projection and display serve: no consent.
     */
    private fun resumeFor(to: String, port: Int, appsOn: Boolean, optIn: Boolean, own: Boolean): Boolean {
        val ok = CastPauseRule.mayResume(pausedFor, to, pausedAt, SystemClock.elapsedRealtime(), appsOn, optIn, own)
        if (!ok || finished || !paused) return false
        main.post {
            if (finished || !paused || pausedFor != to) return@post
            paused = false
            pausedFor = null
            resuming = true
            main.removeCallbacks(pauseExpiry)
            main.removeCallbacks(pauseWatch)
            address = to
            castingTo = to
            screenTo = to
            update(getString(R.string.cast_connecting, shown))
            Thread({ run(port) }, "cast-session").start()
        }
        return true
    }

    /** A pause ended on purpose (a new consent came): the old projection is given back. */
    private fun dropPause() {
        main.removeCallbacks(pauseExpiry)
        main.removeCallbacks(pauseWatch)
        val shownDisplay = display
        val taken = projection
        display = null
        projection = null
        paused = false
        pausedFor = null
        started = false
        shownDisplay?.release()
        Thread { runCatching { taken?.stop() } }.start()
    }

    /** A failure before the computer admitted the cast: a resumed one pauses again, as long as time is left. */
    private fun failOrPause(message: String) {
        if (resuming && !finished && !CastPauseRule.expired(pausedAt, SystemClock.elapsedRealtime())) {
            Log.i(TAG, "The resume did not reach the computer ($message): the pause goes on")
            resuming = false
            castingTo = null
            screenTo = null
            val remaining = CastPauseRule.PAUSE_MS - (SystemClock.elapsedRealtime() - pausedAt)
            main.post {
                if (finished) return@post
                paused = true
                showPaused()
                main.removeCallbacks(pauseExpiry)
                main.removeCallbacks(pauseWatch)
                main.postDelayed(pauseExpiry, remaining.coerceAtLeast(0L))
                main.postDelayed(pauseWatch, CastPauseRule.WATCH_MS)
                Thread {
                    runCatching { RustCore.stopCast() }
                    if (!finished && paused) pausedFor = address
                }.start()
            }
            return
        }
        fail(message)
    }

    private fun fail(message: String) {
        Log.i(TAG, "The cast stopped: $message")
        if (!finished) {
            getSystemService(NotificationManager::class.java)
                .notify(FAILED_NOTIFICATION_ID, failedNotification(message))
        }
        finish()
    }

    /** End it all, once, from any thread. */
    private fun finish() {
        if (finished) return
        finished = true
        // Phone apps tier 2: every app window is given back with the cast.
        windowCast?.closeAll()
        windowCast = null
        pendingWindows.clear()
        castingTo = null
        screenTo = null
        pausedFor = null
        paused = false
        main.removeCallbacks(pauseExpiry)
        main.removeCallbacks(pauseWatch)
        webcamRunning = false
        if (reached) HostStore.markUsed(this, address)
        // Its sound first, off the main thread (its stop waits for its
        // worker): it captures under the projection given back below.
        val capture = sound
        sound = null
        main.post {
            val shown = display
            val coding = encoder
            val taken = projection
            val lens = feed
            display = null
            encoder = null
            projection = null
            feed = null
            shown?.release()
            lens?.close()
            Thread {
                capture?.stop()
                coding?.release()
                runCatching { taken?.stop() }
                runCatching { RustCore.stopCast() }
            }.start()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    override fun onDestroy() {
        if (current === this) current = null
        control?.stop()
        finish()
        super.onDestroy()
    }

    private fun update(text: String) {
        if (finished) return
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(text))
    }

    private fun createChannel() {
        val channel =
            NotificationChannel(CHANNEL_ID, getString(R.string.cast_channel), NotificationManager.IMPORTANCE_LOW)
        channel.setShowBadge(false)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        if (webcam) {
            val camera =
                NotificationChannel(WEBCAM_CHANNEL_ID, getString(R.string.webcam_in_use_channel), NotificationManager.IMPORTANCE_DEFAULT)
            camera.setShowBadge(false)
            camera.setSound(null, null)
            camera.enableVibration(false)
            getSystemService(NotificationManager::class.java).createNotificationChannel(camera)
        }
    }

    private fun notification(text: String): Notification {
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        val stop =
            PendingIntent.getService(
                this,
                1,
                Intent(this, CastService::class.java).setAction(ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        return Notification.Builder(this, if (webcam) WEBCAM_CHANNEL_ID else CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(
                getString(
                    when {
                        soundAlone -> R.string.phone_sound_title
                        webcam -> R.string.webcam_in_use_channel
                        else -> R.string.cast_channel
                    },
                ),
            )
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, R.drawable.ic_notif_disconnect),
                    getString(R.string.cast_stop),
                    stop,
                ).build(),
            ).build()
    }

    private fun failedNotification(text: String): Notification =
        Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(getString(if (soundAlone) R.string.phone_sound_stopped else R.string.cast_stopped))
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .build()
}
