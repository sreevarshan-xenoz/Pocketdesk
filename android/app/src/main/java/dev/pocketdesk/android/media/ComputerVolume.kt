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
import android.os.SystemClock
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONObject

private const val TAG = "ComputerVolume"

/**
 * While [active] (a remote admitted), the phone's volume
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
    // One request at a time, and only the newest wish (K4 review: a held
    // key queued a request per repeat, each its own connection in Remote).
    private val worker = Executors.newSingleThreadExecutor()
    private var session: MediaSession? = null
    @Volatile private var stopped = false

    /** Presses not yet sent, summed. */
    private val delta = AtomicInteger(0)

    /** A step the panel's slider set, not yet sent; -1 for none. */
    private val absolute = AtomicInteger(-1)
    private val draining = AtomicBoolean(false)

    /** The computer's step as last read or set, and when (elapsedRealtime). */
    @Volatile private var known = -1
    @Volatile private var knownAt = 0L

    private val provider =
        object : VolumeProvider(VOLUME_CONTROL_ABSOLUTE, VolumeKeys.STEPS, 0) {
            override fun onAdjustVolume(direction: Int) {
                currentVolume = VolumeKeys.adjust(currentVolume, direction)
                delta.addAndGet(direction.coerceIn(-1, 1))
                drain()
            }

            override fun onSetVolumeTo(volume: Int) {
                currentVolume = volume.coerceIn(0, VolumeKeys.STEPS)
                absolute.set(currentVolume)
                delta.set(0)
                drain()
            }
        }

    fun start() {
        // The panel is offered only once the computer says what its volume
        // is: one that refuses the Audio permission keeps the phone's keys.
        worker.execute {
            val now = status() ?: return@execute
            if (stopped) return@execute
            android.os.Handler(context.mainLooper).post {
                if (stopped) return@post
                provider.currentVolume = now
                session = MediaSession(context, "Pocketdesk volume").apply {
                    // Without a callback the session drops the keys Android
                    // hands it (K4 review): the provider is told through it.
                    setCallback(object : MediaSession.Callback() {}, android.os.Handler(context.mainLooper))
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

    /** Send what was asked, one request at a time, until nothing is left. */
    private fun drain() {
        if (stopped || !draining.compareAndSet(false, true)) return
        worker.execute {
            while (!stopped) {
                val set = absolute.getAndSet(-1)
                val presses = delta.getAndSet(0)
                if (set < 0 && presses == 0) {
                    draining.set(false)
                    // A press that came between the look and the flag.
                    if ((absolute.get() < 0 && delta.get() == 0) || !draining.compareAndSet(false, true)) return@execute
                    continue
                }
                // Presses go from the computer's own volume, read again when
                // the panel may be stale (K4 review: a press after the Sound
                // sheet or the computer moved it jumped it back).
                val target =
                    if (set >= 0) {
                        set
                    } else {
                        val fresh = VolumeKeys.fresh(knownAt, SystemClock.elapsedRealtime())
                        VolumeKeys.target(if (fresh && known >= 0) known else status() ?: known.coerceAtLeast(0), presses)
                    }
                val reply = request(JSONObject().put("action", "setVolume").put("volume", VolumeKeys.toPercent(target)))
                // The computer's answer, or, refused, what it says it is.
                val now = reply?.optInt("volume", -1)?.takeIf { it >= 0 }?.let(VolumeKeys::toStep)
                    ?: if (reply != null) target else status() ?: known
                known = now
                knownAt = SystemClock.elapsedRealtime()
                android.os.Handler(context.mainLooper).post {
                    if (!stopped && absolute.get() < 0 && delta.get() == 0 && now >= 0) provider.currentVolume = now
                }
            }
        }
    }

    /** The computer's volume as a step, or null when it does not say. */
    private fun status(): Int? =
        request(JSONObject().put("action", "status"))
            ?.optInt("volume", -1)
            ?.takeIf { it >= 0 }
            ?.let(VolumeKeys::toStep)
            ?.also {
                known = it
                knownAt = SystemClock.elapsedRealtime()
            }

    private fun request(body: JSONObject): JSONObject? =
        RustCore.audioControl(address, port, body.toString())
            .onFailure { Log.i(TAG, "the computer's volume: ${it.message}") }
            .getOrNull()
            ?.let { runCatching { JSONObject(it) }.getOrNull() }
            ?.takeIf { it.optBoolean("ok", true) && !it.has("error") }
}
