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

        private const val TAG = "CastService"
        private const val CHANNEL_ID = "pocketdesk-cast"
        private const val NOTIFICATION_ID = 7
        private const val FAILED_NOTIFICATION_ID = 8
        private const val POLL_MS = 500L

        /** The owner may be asked; the computer gives them 30 s. */
        private const val WAIT_MS = 45_000L

        /** The computer the screen is shown on now, for the UI; null when none. */
        @Volatile
        var castingTo: String? = null
            private set

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

        /** Start a cast to [address] with the consent Android returned. */
        fun start(context: Context, address: String, port: Int, resultCode: Int, data: Intent) {
            context.startForegroundService(
                Intent(context, CastService::class.java)
                    .setAction(ACTION_START)
                    .putExtra(EXTRA_ADDRESS, address)
                    .putExtra(EXTRA_PORT, port)
                    .putExtra(EXTRA_RESULT_CODE, resultCode)
                    .putExtra(EXTRA_RESULT_DATA, data),
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
    private var started = false

    @Volatile
    private var finished = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            finish()
            return START_NOT_STICKY
        }
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
            started = true
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
                override fun onStop() = finish()
            },
            main,
        )
        castingTo = address
        Thread({ run(port) }, "cast-session").start()
        return START_NOT_STICKY
    }

    /** What the phone's apps play, with its screen (K5). */
    @Volatile
    private var sound: CastSound? = null

    /** Start the cast's sound when everything says it may go (K5). */
    private fun startSound(castState: String) {
        val taken = projection ?: return
        val granted =
            checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
                android.content.pm.PackageManager.PERMISSION_GRANTED
        if (!CastSoundRule.capture(
                Prefs.castSound(this),
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
            RustCore.connectCast(address, port).recoverCatching { lan ->
                if (finished) throw lan
                val identity = HostStore.wanIdentity(this, address) ?: throw lan
                RustCore.connectCastWan(address, identity).getOrThrow()
            }
        if (finished) return
        connected.exceptionOrNull()?.let {
            fail(getString(R.string.cast_failed, shown, plainErrorText(it.message)))
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
                fail(getString(R.string.cast_failed, shown, getString(R.string.cast_no_answer)))
                return
            }
            when (state) {
                State.TooOld -> return fail(getString(R.string.cast_too_old, shown))
                State.Down -> {
                    if (replaced) return finish()
                    if (!showing || closed != null && closed != "session ended") {
                        return fail(getString(R.string.cast_failed, shown, reasonText(closed)))
                    }
                    return finish()
                }
                State.Ready ->
                    if (!showing) {
                        showing = true
                        main.post(::startPicture)
                        startSound(castState)
                        update(getString(if (camera) R.string.cast_showing_camera else R.string.cast_showing, shown))
                        // C8: the computer may control the phone while its screen is shown.
                        if (!camera) control = dev.pocketdesk.android.control.ControlRunner(this) { on ->
                            if (!finished) {
                                update(getString(if (on) R.string.cast_showing_controlled else R.string.cast_showing, shown))
                            }
                        }.also { it.start() }
                    }
                else -> {}
            }
            Thread.sleep(POLL_MS)
        }
    }

    private fun reasonText(closed: String?): String =
        when {
            closed == null -> getString(R.string.cast_lost)
            closed.contains("phone screen not allowed") -> getString(R.string.cast_not_allowed)
            closed.contains("phone screen not shown here") -> getString(R.string.cast_not_shown_here)
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
        val made = CameraFeed(this) { fail(getString(R.string.cast_camera_lost)) }
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
        castingTo = null
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
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(getString(R.string.cast_channel))
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
            .setContentTitle(getString(R.string.cast_stopped))
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .build()
}
