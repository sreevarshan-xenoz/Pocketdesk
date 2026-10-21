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
 * pause, and sends a cover once per link. The listener's 2 s ticks keep the
 * switch in step ([sync]) and take presses ([tick]).
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

    // The worker's.
    private var pending: ScheduledFuture<*>? = null
    private val covers = MediaArt.Cache()
    private var sent = MediaRule.Sent()
    private val sentArt = mutableSetOf<String>()
    private var linkedTo: Pair<String, Int>? = null
    private var pausedSinceMs: Long? = null

    /** Whether a report is current, so the listener keeps a link for it. */
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

    /** Whether the own computer's link is wanted for this: while a report is current. */
    fun linkWanted(): Boolean = active && current

    /**
     * The listener's answers tick: presses, and a report only when one is
     * due again or the link changed; the players' callbacks report changes.
     */
    fun tick() {
        if (!active || worker.isShutdown) return
        worker.execute {
            runCatching {
                takePresses()
                val nowMs = SystemClock.elapsedRealtime()
                // A pause past five minutes is current no longer: no link kept for it.
                if (!MediaRule.current(sent.last, nowMs, pausedSinceMs)) current = false
                val due = current && nowMs - sent.atMs >= MediaRule.RESEND_MS
                if (due || RustCore.controlPeer() != linkedTo) report()
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
        val onChange = MediaSessionManager.OnActiveSessionsChangedListener { controllers -> watch(controllers.orEmpty()) }
        try {
            manager.addOnActiveSessionsChangedListener(onChange, component, main)
            watch(manager.getActiveSessions(component))
        } catch (e: SecurityException) {
            Log.i(TAG, "No notification access: not watching the phone's music")
            return
        }
        listener = onChange
        active = true
        changed()
    }

    private fun unregister() {
        val manager = app.getSystemService(MediaSessionManager::class.java)
        listener?.let { runCatching { manager?.removeOnActiveSessionsChangedListener(it) } }
        listener = null
        watch(emptyList())
        if (!active) return
        active = false
        if (worker.isShutdown) return
        // The computer forgets the song it shows; the app's reporter, if
        // open, says it again in full.
        worker.execute {
            runCatching {
                val own = PhoneLink.target(app)
                if (sent.any && sent.last != null && own != null && linked(own.address, own.controlPort)) {
                    RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_MEDIA, "{}")
                }
            }
            sent = MediaRule.Sent()
            sentArt.clear()
            current = false
        }
    }

    /** Watch [controllers]' changes, and no others (main thread). */
    private fun watch(controllers: List<MediaController>) {
        val tokens = controllers.map { it.sessionToken }.toSet()
        watched.keys.filter { it !in tokens }.forEach { token ->
            watched.remove(token)?.let { (controller, callback) -> controller.unregisterCallback(callback) }
        }
        for (controller in controllers) {
            if (controller.sessionToken in watched) continue
            val callback = object : MediaController.Callback() {
                override fun onPlaybackStateChanged(state: PlaybackState?) = changed()
                override fun onMetadataChanged(metadata: MediaMetadata?) = changed()
                override fun onSessionDestroyed() = changed()
            }
            controller.registerCallback(callback, main)
            watched[controller.sessionToken] = controller to callback
        }
        if (active) changed()
    }

    /** Report once things settle: a burst of changes is one report. */
    private fun changed() {
        if (worker.isShutdown) return
        worker.execute {
            pending?.cancel(false)
            pending = worker.schedule(
                { runCatching { report() }.onFailure { Log.w(TAG, "report failed", it) } },
                SETTLE_MS,
                TimeUnit.MILLISECONDS,
            )
        }
    }

    private fun linked(address: String, port: Int): Boolean =
        !ControlLink.needsConnect(RustCore.controlPeer(), address, port)

    /** Presses from the computer, acted on the session reported (worker). */
    private fun takePresses() {
        val presses = RustCore.takeMediaPresses()
        if (presses.isEmpty()) return
        val target = sent.last?.packageName ?: return
        val controller = MediaSessions.controllers(app).firstOrNull { it.packageName == target } ?: return
        for (command in presses) {
            if (MediaRule.accepts(command, wanted())) MediaSessions.press(controller, command)
        }
        // The player's callbacks report its new state.
    }

    /** Send what plays when [MediaRule.backgroundDue] says so (worker). */
    private fun report() {
        if (!active) return
        val own = PhoneLink.target(app) ?: return
        val peer = RustCore.controlPeer()
        val linked = linked(own.address, own.controlPort)
        // A link to another computer, or a new one, knows nothing of what was sent.
        if (peer != linkedTo || !linked) {
            linkedTo = peer
            sent = MediaRule.Sent()
            sentArt.clear()
        }
        val controllers = MediaSessions.controllers(app)
        val picked = MediaRule.pick(controllers.mapNotNull { MediaSessions.describe(app, it) })
        val cover = picked?.let { media ->
            covers.of(media.packageName, controllers.firstOrNull { it.packageName == media.packageName }?.metadata)
        }
        val now = picked?.copy(artId = cover?.id)
        val nowMs = SystemClock.elapsedRealtime()
        pausedSinceMs = when {
            now == null || now.isPlaying -> null
            else -> pausedSinceMs ?: nowMs
        }
        current = MediaRule.current(now, nowMs, pausedSinceMs)
        if (!linked || !MediaRule.backgroundDue(sent, now, nowMs, pausedSinceMs)) return
        val withArt = cover?.takeIf { MediaRule.sendArt(it.id, sentArt) }
        val body = MediaSessions.body(now, background = true, cover = withArt)
        val result = RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_MEDIA, body.toString())
        if (result.isSuccess) {
            sent = MediaRule.Sent(any = true, last = now, atMs = nowMs)
            withArt?.let { sentArt += it.id }
        } else {
            Log.d(TAG, "Not sent: ${result.exceptionOrNull()?.message}")
        }
    }
}
