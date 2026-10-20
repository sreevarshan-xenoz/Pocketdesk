package dev.pocketdesk.android.media

import android.content.Context
import android.media.VolumeProvider
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import java.util.concurrent.Executors
import org.json.JSONObject

private const val TAG = "ComputerVolume"

/**
 * While [active] (a remote or sound session admitted), the phone's volume
 * keys and Android's volume panel change the computer's volume (K4): a
 * media session whose playback is remote, as casting apps use. Released
 * when it ends. Never with Settings "Volume keys change the computer's
 * volume" off, nor for a computer that refuses the Audio permission.
 */
@Composable
fun ComputerVolumeEffect(address: String, controlPort: Int, active: Boolean) {
    val context = LocalContext.current
    DisposableEffect(address, controlPort, active) {
        val volume = if (active && Prefs.volumeKeys(context)) ComputerVolume(context, address, controlPort) else null
        volume?.start()
        onDispose { volume?.stop() }
    }
}

/** One media session for one computer's volume. */
private class ComputerVolume(private val context: Context, private val address: String, private val port: Int) {
    // Requests one at a time, in order: a held key sends many.
    private val worker = Executors.newSingleThreadExecutor()
    private var session: MediaSession? = null
    @Volatile private var stopped = false

    private val provider =
        object : VolumeProvider(VOLUME_CONTROL_ABSOLUTE, VolumeKeys.STEPS, 0) {
            override fun onAdjustVolume(direction: Int) {
                set(VolumeKeys.adjust(currentVolume, direction))
            }

            override fun onSetVolumeTo(volume: Int) {
                set(volume.coerceIn(0, VolumeKeys.STEPS))
            }
        }

    fun start() {
        // The panel is offered only once the computer says what its volume
        // is: one that refuses the Audio permission keeps the phone's keys.
        worker.execute {
            val now = request(JSONObject().put("action", "status"))?.optInt("volume", -1) ?: -1
            if (now < 0 || stopped) return@execute
            android.os.Handler(context.mainLooper).post {
                if (stopped) return@post
                provider.currentVolume = VolumeKeys.toStep(now)
                session = MediaSession(context, "Pocketdesk volume").apply {
                    setPlaybackToRemote(provider)
                    // Android gives the keys to a session that plays.
                    setPlaybackState(
                        PlaybackState.Builder()
                            .setState(PlaybackState.STATE_PLAYING, 0L, 1f)
                            .build(),
                    )
                    setMetadata(
                        android.media.MediaMetadata.Builder()
                            .putString(android.media.MediaMetadata.METADATA_KEY_TITLE, HostStore.shownName(context, address))
                            .build(),
                    )
                    isActive = true
                }
            }
        }
    }

    fun stop() {
        stopped = true
        session?.let {
            it.isActive = false
            it.release()
        }
        session = null
        worker.shutdown()
    }

    private fun set(step: Int) {
        provider.currentVolume = step
        if (stopped) return
        worker.execute {
            if (stopped) return@execute
            request(JSONObject().put("action", "setVolume").put("volume", VolumeKeys.toPercent(step)))
        }
    }

    private fun request(body: JSONObject): JSONObject? =
        RustCore.audioControl(address, port, body.toString())
            .onFailure { Log.i(TAG, "the computer's volume: ${it.message}") }
            .getOrNull()
            ?.let { runCatching { JSONObject(it) }.getOrNull() }
            ?.takeIf { it.optBoolean("ok", true) && !it.has("error") }
}
