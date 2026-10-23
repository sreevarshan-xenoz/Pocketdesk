package dev.pocketdesk.android.media

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.ui.ComputerPlayers
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import org.json.JSONObject
import java.util.concurrent.Executors

private const val TAG = "ComputerMedia"

/** Phone → computer: tell me what plays, or stop (`core::protocol::computer_media::WATCH`). */
private const val WATCH = "kdeconnect.pocketdesk.computer_media_watch"

/** The largest cover taken, in bytes, as the computer sends at most. */
private const val MAX_COVER_BYTES = 256 * 1024

/** The longest side a cover is drawn at. */
private const val MAX_COVER_PX = 1024

/**
 * The computer's music in the phone's media controls (N5): with "Computer's
 * music in my media controls" on, the notification listener asks its own
 * computer to tell it what plays, and shows it as a media session of its
 * own with a media notification, so it is on the lock screen, in the
 * shade's media controls and on a watch. A press there goes back as T8c's
 * `mediaControl`. All of it runs on the listener's 2 s answers tick
 * ([tick]); what shows ends as [ComputerMediaRule] says.
 */
class ComputerMedia(private val app: Context) {
    companion object {
        const val CHANNEL = "computer_media"
        const val ACTION_PRESS = "dev.pocketdesk.android.action.COMPUTER_MEDIA"
        const val EXTRA_COMMAND = "command"
        private const val NOTIFICATION_ID = 0x4E05

        @Volatile
        private var instance: ComputerMedia? = null

        /** A press on the notification's buttons, sent if the listener runs; else its stale notification goes. */
        fun press(context: Context, command: String) {
            val running = instance
            if (running != null) {
                running.press(command)
            } else {
                context.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
            }
        }
    }

    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()

    // The worker's.
    private val shown = ComputerMediaRule.Shown()
    private val watch = ComputerMediaRule.Watch()
    private var heardMs: Long? = null
    private var cover: Pair<String, Bitmap>? = null

    /** A cover whose bytes would not decode: not asked for again (N5 review). */
    private var badCover: String? = null
    private var showingFor: String? = null

    /** The player the last watch named (O6). */
    private var watchedPlayer: String? = null

    /** Closed with the listener: nothing is drawn after (N5 review). */
    @Volatile
    private var closed = false

    // The main thread's.
    private var session: MediaSession? = null
    private var drawn: Pair<ComputerMediaRule.Playing, Bitmap?>? = null

    init {
        instance = this
    }

    private fun wanted(): Boolean = Prefs.computerMediaInControls(app)

    /** Whether the own computer's link is wanted for this: while the switch is on. */
    fun linkWanted(): Boolean = wanted()

    /** The listener's answers tick: ask, take what the computer pushed, show it or end it. */
    fun tick() {
        if (worker.isShutdown) return
        runCatching { worker.execute { runCatching { step() }.onFailure { Log.w(TAG, "tick failed", it) } } }
    }

    fun close() {
        closed = true
        if (instance === this) instance = null
        runCatching {
            worker.execute {
                // Told to stop where it was asked, so the computer stops reading.
                val own = PhoneLink.target(app)
                val link = own?.let { live(it) }
                if (own != null && link != null && watch.due(link, wanted = false) == false) {
                    send(own, false)
                }
                // Last in the queue: after any draw a tick before it posted.
                main.post { end() }
            }
        }
        worker.shutdown()
        main.post { end() }
    }

    private fun live(own: HostStore.Host): Pair<String, Int>? =
        RustCore.controlPeer()?.takeIf { it == own.address to own.controlPort }

    private fun send(own: HostStore.Host, on: Boolean): Boolean =
        RustCore.sendPhonePacketOnLink(
            own.address,
            own.controlPort,
            WATCH,
            JSONObject().put("on", on).apply { HostStore.player(app, own.address)?.let { put("player", it) } }.toString(),
        )
            .onFailure { Log.d(TAG, "The watch was not sent: ${it.message}") }
            .isSuccess

    /** One tick's work (worker). */
    private fun step() {
        if (closed) return
        val nowMs = SystemClock.elapsedRealtime()
        val own = PhoneLink.target(app)
        val wanted = wanted() && own != null
        // A new own computer, or the switch off: what showed goes.
        if (!wanted || showingFor != own?.address) {
            shown.clear()
            showingFor = own?.address
            heardMs = null
        }
        RustCore.takeComputerMedia()?.let { push ->
            val host = push.optString("host")
            val port = push.optInt("port", -1)
            // O6: one read for a player chosen before is not shown (O6 review).
            val pushed = if (push.isNull("player")) null else push.optString("player")
            val forChosen = ComputerPlayers.pushFor(push.has("player"), pushed, own?.let { HostStore.player(app, it.address) })
            if (wanted && forChosen && ComputerMediaRule.fromOwn(host, port, own?.address, own?.controlPort)) heard(push, nowMs)
        }
        val current = shown.current(nowMs)
        val link = own?.let { live(it) }
        val player = own?.let { HostStore.player(app, it.address) }
        if (link == null) watchedPlayer = null
        val coverMissing = current?.artId != null && cover?.first != current.artId && badCover != current.artId
        // O6: a new choice asks again with it, the link's rule unchanged.
        val due = watch.due(link, wanted, nowMs, heardMs, coverMissing)
            ?: true.takeIf { wanted && link != null && player != watchedPlayer }
        due?.let { on ->
            if (own != null && link != null && send(own, on)) {
                watch.sent(link, on, nowMs)
                watchedPlayer = if (on) player else null
            }
        }
        val art = cover?.takeIf { it.first == current?.artId }?.second
        val computer = own?.let { HostStore.shownName(app, it.address) }.orEmpty()
        main.post { draw(current, art, computer) }
    }

    /** A push from the own computer (worker). */
    private fun heard(push: JSONObject, nowMs: Long) {
        heardMs = nowMs
        val playing = push.optJSONObject("playing")
        val artId = ComputerMediaRule.artId(push.optString("artId").takeIf { it.isNotEmpty() })
        if (artId != null && cover?.first != artId && push.has("art")) {
            val decoded = decodeCover(push.optString("art"))
            if (decoded != null) cover = artId to decoded else badCover = artId
        }
        shown.heard(
            playing?.let {
                ComputerMediaRule.Playing(
                    title = it.optString("title"),
                    artist = it.optString("artist"),
                    app = it.optString("app"),
                    isPlaying = it.optBoolean("isPlaying"),
                    canPlayPause = it.optBoolean("canPlayPause"),
                    canNext = it.optBoolean("canNext"),
                    canPrevious = it.optBoolean("canPrevious"),
                    artId = artId,
                )
            }?.takeIf { it.title.isNotBlank() },
            nowMs,
        )
    }

    /**
     * A cover's base64, as a picture when it is one of at most
     * [MAX_COVER_BYTES], drawn at most [MAX_COVER_PX] a side whatever size
     * it declares (N5 review).
     */
    private fun decodeCover(base64: String): Bitmap? = runCatching {
        if (base64.length > MAX_COVER_BYTES / 3 * 4 + 4) return null
        val bytes = android.util.Base64.decode(base64, android.util.Base64.DEFAULT)
        if (bytes.size > MAX_COVER_BYTES) return null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_COVER_PX) sample *= 2
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
    }.getOrNull()

    /** Send a press to the own computer (worker). */
    private fun press(command: String) {
        if (command !in setOf(ComputerMediaRule.PLAY_PAUSE, ComputerMediaRule.NEXT, ComputerMediaRule.PREVIOUS)) return
        runCatching {
            worker.execute {
                if (!wanted()) return@execute
                val own = PhoneLink.target(app) ?: return@execute
                val body = JSONObject().put("action", "mediaControl").put("command", command)
                HostStore.player(app, own.address)?.let { body.put("player", it) }
                RustCore.audioControl(own.address, own.controlPort, body.toString())
                    .onFailure { Log.i(TAG, "A press did not reach the computer: ${it.message}") }
                // The computer pushes its new state within 2 s.
            }
        }
    }

    private val callback = object : MediaSession.Callback() {
        // The computer takes one toggle: sent only when it changes what shows.
        override fun onPlay() {
            if (ComputerMediaRule.toggles(true, drawn?.first?.isPlaying)) press(ComputerMediaRule.PLAY_PAUSE)
        }

        override fun onPause() {
            if (ComputerMediaRule.toggles(false, drawn?.first?.isPlaying)) press(ComputerMediaRule.PLAY_PAUSE)
        }

        override fun onSkipToNext() = press(ComputerMediaRule.NEXT)

        override fun onSkipToPrevious() = press(ComputerMediaRule.PREVIOUS)
    }

    /** Show [now] with [art], or end what shows (main thread). */
    private fun draw(now: ComputerMediaRule.Playing?, art: Bitmap?, computer: String) {
        if (now == null || closed) {
            end()
            return
        }
        if (drawn?.first == now && drawn?.second === art && session != null) return
        val s = session ?: MediaSession(app, "Pocketdesk computer").also {
            it.setCallback(callback, main)
            session = it
        }
        s.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, now.title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, now.artist)
                .putString(MediaMetadata.METADATA_KEY_ALBUM, now.app)
                .apply { art?.let { putBitmap(MediaMetadata.METADATA_KEY_ART, it) } }
                .build(),
        )
        var actions = 0L
        if (now.canPlayPause) actions = actions or PlaybackState.ACTION_PLAY_PAUSE or PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE
        if (now.canNext) actions = actions or PlaybackState.ACTION_SKIP_TO_NEXT
        if (now.canPrevious) actions = actions or PlaybackState.ACTION_SKIP_TO_PREVIOUS
        s.setPlaybackState(
            PlaybackState.Builder()
                .setActions(actions)
                .setState(
                    if (now.isPlaying) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                    PlaybackState.PLAYBACK_POSITION_UNKNOWN,
                    if (now.isPlaying) 1f else 0f,
                )
                .build(),
        )
        s.isActive = true
        notify(now, art, computer, s.sessionToken)
        drawn = now to art
    }

    /** The media notification for [now] (main thread). */
    private fun notify(now: ComputerMediaRule.Playing, art: Bitmap?, computer: String, token: MediaSession.Token) {
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        if (!manager.areNotificationsEnabled()) return
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, app.getString(R.string.channel_computer_media), NotificationManager.IMPORTANCE_LOW)
                    .apply { setShowBadge(false) },
            )
        }
        val buttons = buildList {
            if (now.canPrevious) add(Triple(ComputerMediaRule.PREVIOUS, R.drawable.ic_ll_skip_previous, R.string.media_previous))
            if (now.canPlayPause) {
                add(
                    if (now.isPlaying) {
                        Triple(ComputerMediaRule.PLAY_PAUSE, R.drawable.ic_ll_pause, R.string.media_pause)
                    } else {
                        Triple(ComputerMediaRule.PLAY_PAUSE, R.drawable.ic_ll_play, R.string.media_play)
                    },
                )
            }
            if (now.canNext) add(Triple(ComputerMediaRule.NEXT, R.drawable.ic_ll_skip_next, R.string.media_next))
        }
        val builder = Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(now.title)
            .setContentText(now.artist.ifBlank { now.app })
            .setSubText(computer.ifBlank { null })
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setOngoing(now.isPlaying)
        art?.let { builder.setLargeIcon(it) }
        app.packageManager.getLaunchIntentForPackage(app.packageName)?.let { launch ->
            builder.setContentIntent(PendingIntent.getActivity(app, 0, launch, PendingIntent.FLAG_IMMUTABLE))
        }
        buttons.forEachIndexed { i, (command, icon, label) ->
            val intent = Intent(app, ComputerMediaReceiver::class.java).setAction(ACTION_PRESS).putExtra(EXTRA_COMMAND, command)
            val pending = PendingIntent.getBroadcast(app, i, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            builder.addAction(Notification.Action.Builder(Icon.createWithResource(app, icon), app.getString(label), pending).build())
        }
        builder.setStyle(
            Notification.MediaStyle()
                .setMediaSession(token)
                .setShowActionsInCompactView(*buttons.indices.take(3).toIntArray()),
        )
        runCatching { manager.notify(NOTIFICATION_ID, builder.build()) }
            .onFailure { Log.w(TAG, "The computer's music was not shown: ${it.message}") }
    }

    /** End the session and its notification (main thread). */
    private fun end() {
        if (session == null && drawn == null) return
        session?.run {
            isActive = false
            release()
        }
        session = null
        drawn = null
        app.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }
}

/** The media notification's buttons (N5): a press goes to the computer through [ComputerMedia]. */
class ComputerMediaReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ComputerMedia.ACTION_PRESS) return
        val command = intent.getStringExtra(ComputerMedia.EXTRA_COMMAND) ?: return
        ComputerMedia.press(context.applicationContext, command)
    }
}
