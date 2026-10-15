package dev.pocketdesk.android.media

import android.content.ComponentName
import android.content.Context
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneNotificationListener
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.ui.ControlLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import org.json.JSONObject

private const val PHONE_MEDIA = "kdeconnect.pocketdesk.phone_media"
private const val CHECK_MS = 2_000L
private const val TAG = "PhoneMedia"

/**
 * Tells the computer of this session what the phone plays (T8d) and acts on
 * its presses, while "My media on the computer" is on and Android lets this
 * app read media sessions (notification access). It checks every 2 s while
 * the app is started, and sends only on the session's own control link.
 */
@Composable
fun PhoneMediaEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        var sent = MediaRule.Sent()
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (isActive) {
                withContext(Dispatchers.IO) {
                    val on = Prefs.phoneMediaOnComputer(context) &&
                        PhoneNotificationPrefs.accessGranted(context)
                    val linked = !ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)
                    // A computer reached again (a restart, a dropped link)
                    // knows nothing of what was sent: it hears it again.
                    sent = MediaRule.forLink(sent, linked)
                    val presses = RustCore.takeMediaPresses()
                    var controllers = if (on) controllers(context) else emptyList()
                    // Presses act on the session reported, the one the computer shows.
                    for (command in presses) {
                        if (!MediaRule.accepts(command, on)) continue
                        val target = sent.last?.packageName ?: continue
                        controllers.firstOrNull { it.packageName == target }?.let { press(it, command) }
                    }
                    if (linked) {
                        if (on && presses.isNotEmpty()) {
                            delay(300) // the player's new state
                            controllers = controllers(context)
                        }
                        // With the setting off, `{}` once, so the computer
                        // forgets the song it shows.
                        val now = if (on) MediaRule.pick(controllers.mapNotNull { describe(context, it) }) else null
                        val nowMs = android.os.SystemClock.elapsedRealtime()
                        if (MediaRule.shouldSend(sent, now, nowMs)) {
                            val body = now?.let { json(it) } ?: JSONObject()
                            val result = RustCore.sendPhonePacket(address, controlPort, PHONE_MEDIA, body.toString())
                            if (result.isSuccess) {
                                sent = MediaRule.Sent(any = true, last = now, atMs = nowMs)
                            } else {
                                Log.d(TAG, "Not sent: ${result.exceptionOrNull()?.message}")
                            }
                        }
                    }
                }
                delay(CHECK_MS)
            }
        }
    }
}

/** The active media sessions, most recent first; none without notification access. */
private fun controllers(context: Context): List<MediaController> {
    val manager = context.getSystemService(MediaSessionManager::class.java) ?: return emptyList()
    return try {
        manager.getActiveSessions(ComponentName(context, PhoneNotificationListener::class.java))
    } catch (e: SecurityException) {
        emptyList()
    }
}

private fun describe(context: Context, controller: MediaController): MediaRule.Media? {
    val state = controller.playbackState ?: return null
    val metadata = controller.metadata
    val actions = state.actions
    val app = runCatching {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(controller.packageName, 0)).toString()
    }.getOrDefault(controller.packageName)
    fun can(action: Long) = actions and action != 0L
    return MediaRule.Media(
        title = metadata?.getString(android.media.MediaMetadata.METADATA_KEY_TITLE).orEmpty(),
        artist = metadata?.getString(android.media.MediaMetadata.METADATA_KEY_ARTIST).orEmpty(),
        app = app,
        packageName = controller.packageName,
        isPlaying = state.state == PlaybackState.STATE_PLAYING,
        canPlayPause = can(PlaybackState.ACTION_PLAY_PAUSE) ||
            can(PlaybackState.ACTION_PLAY) || can(PlaybackState.ACTION_PAUSE),
        canNext = can(PlaybackState.ACTION_SKIP_TO_NEXT),
        canPrevious = can(PlaybackState.ACTION_SKIP_TO_PREVIOUS),
    )
}

private fun json(media: MediaRule.Media): JSONObject = JSONObject()
    .put("title", media.title)
    .put("artist", media.artist)
    .put("app", media.app)
    .put("isPlaying", media.isPlaying)
    .put("canPlayPause", media.canPlayPause)
    .put("canNext", media.canNext)
    .put("canPrevious", media.canPrevious)

private fun press(controller: MediaController, command: String) {
    val controls = controller.transportControls
    when (command) {
        "playPause" ->
            if (controller.playbackState?.state == PlaybackState.STATE_PLAYING) controls.pause() else controls.play()
        "next" -> controls.skipToNext()
        "previous" -> controls.skipToPrevious()
    }
}
