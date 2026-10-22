package dev.pocketdesk.android.media

import android.content.ComponentName
import android.content.Context
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSession
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.notify.PhoneNotificationListener
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.ui.ControlLink
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

private const val TAG = "BackgroundMedia"

/** How long a burst of player changes is let settle before one report. */
private const val SETTLE_MS = 500L

/**
 * The phone's music for its own computer without a session (N2): with "My
 * media on the computer in the background" on, the notification listener
 * watches the media sessions (Android's callbacks, no polling) and reports
 * what plays on the own computer's live link only, never dialing for it.
 * It says it again each minute while it plays, or for five minutes after a
 * pause, with the cover when the computer does not hold it. The listener's
 * 2 s ticks keep the switch in step ([sync]) and take presses ([tick]).
 */
class BackgroundMedia(private val app: Context) {
    companion object {
        /**
         * Whether this reporter is registered: the app's own reporter steps
         * aside only then, not merely while the switch is on, so a listener
         * that is frozen or unbound leaves a session's reporter at work.
         */
        @Volatile
        var active = false
            private set
    }

    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadScheduledExecutor()
    private val component = ComponentName(app, PhoneNotificationListener::class.java)

    // The main thread's.
    private var listener: MediaSessionManager.OnActiveSessionsChangedListener? = null
    private val watched = HashMap<MediaSession.Token, Pair<MediaController, MediaController.Callback>>()

    /** What each player last said of its state, so a repeat is let go (main thread). */
    private val lastState = HashMap<MediaSession.Token, Pair<Int, Long>>()

    /**
     * Each session's metadata as its callbacks told it: asking the player
     * again carries its cover's bitmap each time (N2 review).
     */
    private val metadata = ConcurrentHashMap<MediaSession.Token, MediaMetadata>()

    // The worker's.
    private var pending: ScheduledFuture<*>? = null
    private val covers = MediaArt.Cache()
    private val report = BackgroundReport()
    private var cover: MediaArt.Cover? = null

    /** Whether a report is current, so the listener keeps a link for it (any thread). */
    @Volatile
    private var current = false

    private fun wanted(): Boolean =
        Prefs.phoneMediaInBackground(app) && Prefs.phoneMediaOnComputer(app) &&
            PhoneNotificationPrefs.accessGranted(app)

    /** Register or unregister as the switch says; cheap enough for the keeper's 2 s tick. */
    fun sync() {
        val wanted = wanted()
        if (wanted == active) return
        main.post { if (wanted) register() else unregister() }
    }

    /** Whether the own computer's link is wanted for this: while what plays is current. */
    fun linkWanted(): Boolean = active && current

    /**
     * The listener's answers tick: presses, a pause aging out, and a report
     * when the link changed or one is due again; changes come by callbacks.
     */
    fun tick() {
        if (!active || worker.isShutdown) return
        worker.execute {
            runCatching {
                takePresses()
                val nowMs = SystemClock.elapsedRealtime()
                current = report.current(nowMs)
                send(nowMs)
            }.onFailure { Log.w(TAG, "tick failed", it) }
        }
    }

    fun close() {
        main.post {
            unregister()
            worker.shutdown()
        }
    }

    private fun register() {
        if (listener != null || !wanted()) return
        val manager = app.getSystemService(MediaSessionManager::class.java) ?: return
        lateinit var onChange: MediaSessionManager.OnActiveSessionsChangedListener
        onChange = MediaSessionManager.OnActiveSessionsChangedListener { controllers ->
            // One queued before an unregister is let go (N2 review).
            if (listener === onChange) watch(MediaSessions.others(app, controllers.orEmpty()))
        }
        try {
            manager.addOnActiveSessionsChangedListener(onChange, component, main)
        } catch (e: SecurityException) {
            Log.i(TAG, "No notification access: not watching the phone's music")
            return
        }
        listener = onChange
        val controllers = try {
            manager.getActiveSessions(component)
        } catch (e: SecurityException) {
            // Not left registered for the next try to add another (N2 review).
            runCatching { manager.removeOnActiveSessionsChangedListener(onChange) }
            listener = null
            return
        }
        active = true
        watch(MediaSessions.others(app, controllers))
    }

    private fun unregister() {
        val manager = app.getSystemService(MediaSessionManager::class.java)
        listener?.let { runCatching { manager?.removeOnActiveSessionsChangedListener(it) } }
        listener = null
        watch(emptyList())
        if (!active) return
        active = false
        current = false
        if (worker.isShutdown) return
        // The computer forgets the song it shows; the app's reporter, if
        // open, says it again in full.
        worker.execute {
            runCatching {
                val own = PhoneLink.target(app)
                if (report.sent.last != null && own != null && linked(own.address, own.controlPort)) {
                    RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_MEDIA, "{}")
                }
            }
            report.forget()
            cover = null
        }
    }

    /** Watch [controllers]' changes, and no others (main thread). */
    private fun watch(controllers: List<MediaController>) {
        val tokens = controllers.map { it.sessionToken }.toSet()
        watched.keys.filter { it !in tokens }.forEach { token ->
            watched.remove(token)?.let { (controller, callback) -> controller.unregisterCallback(callback) }
            lastState.remove(token)
            metadata.remove(token)
        }
        for (controller in controllers) {
            val token = controller.sessionToken
            if (token in watched) continue
            controller.metadata?.let { metadata[token] = it }
            val callback = object : MediaController.Callback() {
                override fun onPlaybackStateChanged(state: PlaybackState?) {
                    // Players that set their state each second for its
                    // position are let go: only the state and what can be
                    // pressed are reported (N2 review).
                    val now = (state?.state ?: -1) to (state?.actions ?: 0L)
                    if (lastState.put(token, now) != now) changed()
                }

                override fun onMetadataChanged(data: MediaMetadata?) {
                    if (data == null) metadata.remove(token) else metadata[token] = data
                    changed()
                }

                override fun onSessionDestroyed() = changed()
            }
            controller.registerCallback(callback, main)
            watched[token] = controller to callback
            lastState[token] = (controller.playbackState?.state ?: -1) to (controller.playbackState?.actions ?: 0L)
        }
        if (active) changed()
    }

    /** Report once things settle: a burst of changes is one report. */
    private fun changed() {
        if (!active || worker.isShutdown) return
        runCatching {
            worker.execute {
                pending?.cancel(false)
                pending = worker.schedule(
                    { runCatching { look() }.onFailure { Log.w(TAG, "report failed", it) } },
                    SETTLE_MS,
                    TimeUnit.MILLISECONDS,
                )
            }
        }
    }

    private fun linked(address: String, port: Int): Boolean =
        !ControlLink.needsConnect(RustCore.controlPeer(), address, port)

    /** Presses from the computer, acted on the session reported (worker). */
    private fun takePresses() {
        val presses = RustCore.takeMediaPresses()
        if (presses.isEmpty()) return
        val target = report.sent.last?.packageName ?: return
        val controller = MediaSessions.controllers(app).firstOrNull { it.packageName == target } ?: return
        val on = wanted()
        for (command in presses) {
            if (MediaRule.accepts(command, on)) MediaSessions.press(controller, command)
        }
        // The player's callbacks report its new state.
    }

    /** What plays now, its cover and since when it is paused, then a report if due (worker). */
    private fun look() {
        if (!active) return
        val sessions = MediaSessions.controllers(app).mapNotNull { controller ->
            val data = metadata[controller.sessionToken]
            MediaSessions.describe(app, controller, data)?.let { Triple(controller, data, it) }
        }
        val picked = MediaRule.pick(sessions.map { it.third })
        // The cover of the session picked, not another of its app's (N2 review).
        val chosen = sessions.firstOrNull { it.third === picked }
        cover = chosen?.let { (controller, data, _) -> covers.of(controller.packageName, data) }
        val nowMs = SystemClock.elapsedRealtime()
        // Since when it is paused by the player's own clock, so a song paused
        // yesterday is not current after a restart (N2 review).
        val since = chosen?.first?.playbackState?.lastPositionUpdateTime?.takeIf { it in 1..nowMs } ?: nowMs
        report.see(picked?.copy(artId = cover?.id), since)
        current = report.current(nowMs)
        send(nowMs)
    }

    /** Send what plays when the link is up and a report is due (worker). */
    private fun send(nowMs: Long) {
        val own = PhoneLink.target(app) ?: return
        val linked = linked(own.address, own.controlPort)
        report.link(RustCore.controlPeer(), linked)
        if (!linked || !report.due(nowMs)) return
        val withArt = cover?.takeIf { report.withArt() && it.id == report.seen?.artId }
        val body = MediaSessions.body(report.seen, background = true, cover = withArt)
        val result = RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_MEDIA, body.toString())
        if (result.isSuccess) {
            report.sent(nowMs)
        } else {
            Log.d(TAG, "Not sent: ${result.exceptionOrNull()?.message}")
        }
    }
}
