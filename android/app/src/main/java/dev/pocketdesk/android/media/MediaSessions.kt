package dev.pocketdesk.android.media

import android.content.ComponentName
import android.content.Context
import android.graphics.Bitmap
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.util.Base64
import androidx.core.graphics.scale
import dev.pocketdesk.android.notify.PhoneNotificationListener
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.security.MessageDigest

/** The packet both reporters send (T8d). */
internal const val PHONE_MEDIA = "kdeconnect.pocketdesk.phone_media"

/**
 * What both of the phone's media reporters share: the app's own, while it
 * is open (T8d), and the listener's in the background (N2).
 */
internal object MediaSessions {
    /**
     * The active media sessions, most recent first, never this app's own
     * (N5: the computer's music shown here must not go back to it as the
     * phone's); none without notification access.
     */
    fun controllers(context: Context): List<MediaController> {
        val manager = context.getSystemService(MediaSessionManager::class.java) ?: return emptyList()
        return try {
            others(context, manager.getActiveSessions(ComponentName(context, PhoneNotificationListener::class.java)))
        } catch (e: SecurityException) {
            emptyList()
        }
    }

    /** [controllers] without this app's own sessions (N5). */
    fun others(context: Context, controllers: List<MediaController>): List<MediaController> =
        controllers.filter { it.packageName != context.packageName }

    /** Apps' names by package, looked up once (a report asks for each session's). */
    private val labels = java.util.concurrent.ConcurrentHashMap<String, String>()

    private fun label(context: Context, packageName: String): String =
        labels[packageName] ?: runCatching {
            val pm = context.packageManager
            pm.getApplicationLabel(pm.getApplicationInfo(packageName, 0)).toString()
        }.getOrDefault(packageName).also { if (labels.size < 64) labels[packageName] = it }

    /**
     * [controller]'s session as reported, from [metadata] (by default asked
     * of the player, a call that carries its cover's bitmap: the listener
     * passes what its callbacks were told).
     */
    fun describe(
        context: Context,
        controller: MediaController,
        metadata: MediaMetadata? = controller.metadata,
    ): MediaRule.Media? {
        val state = controller.playbackState ?: return null
        val actions = state.actions
        val app = label(context, controller.packageName)
        fun can(action: Long) = actions and action != 0L
        return MediaRule.Media(
            title = metadata?.getString(MediaMetadata.METADATA_KEY_TITLE).orEmpty(),
            artist = metadata?.getString(MediaMetadata.METADATA_KEY_ARTIST).orEmpty(),
            app = app,
            packageName = controller.packageName,
            isPlaying = state.state == PlaybackState.STATE_PLAYING,
            canPlayPause = can(PlaybackState.ACTION_PLAY_PAUSE) ||
                can(PlaybackState.ACTION_PLAY) || can(PlaybackState.ACTION_PAUSE),
            canNext = can(PlaybackState.ACTION_SKIP_TO_NEXT),
            canPrevious = can(PlaybackState.ACTION_SKIP_TO_PREVIOUS),
        )
    }

    /**
     * The report's body for [media] (`{}` for none): with the cover's id,
     * and its bytes when [cover] is given, and `background` for the
     * listener's reporter (N2).
     */
    fun body(media: MediaRule.Media?, background: Boolean, cover: MediaArt.Cover?): JSONObject {
        media ?: return JSONObject()
        val body = JSONObject()
            .put("title", media.title)
            .put("artist", media.artist)
            .put("app", media.app)
            .put("isPlaying", media.isPlaying)
            .put("canPlayPause", media.canPlayPause)
            .put("canNext", media.canNext)
            .put("canPrevious", media.canPrevious)
        if (background) body.put("background", true)
        media.artId?.let { body.put("artId", it) }
        if (cover != null && cover.id == media.artId) {
            body.put("art", Base64.encodeToString(cover.jpeg, Base64.NO_WRAP))
        }
        return body
    }

    fun press(controller: MediaController, command: String) {
        val controls = controller.transportControls
        when (command) {
            "playPause" ->
                if (controller.playbackState?.state == PlaybackState.STATE_PLAYING) controls.pause() else controls.play()
            "next" -> controls.skipToNext()
            "previous" -> controls.skipToPrevious()
        }
    }
}

/**
 * A player's cover, as the computer keeps it (N2): at most [MAX_SIDE] a
 * side, a JPEG, named by its bytes' SHA-256.
 */
object MediaArt {
    const val MAX_SIDE = 256
    private const val QUALITY = 80

    class Cover(val id: String, val jpeg: ByteArray)

    /** The id the computer checks a cover by: the first 16 hex digits of its SHA-256. */
    fun id(jpeg: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(jpeg).take(8).joinToString("") { "%02x".format(it) }

    /** [width] x [height] scaled to fit [MAX_SIDE], never up. */
    fun fit(width: Int, height: Int): Pair<Int, Int> {
        val longest = maxOf(width, height)
        if (longest <= MAX_SIDE) return width to height
        return maxOf(1, width * MAX_SIDE / longest) to maxOf(1, height * MAX_SIDE / longest)
    }

    /**
     * The cover of one player's metadata, encoded once while its metadata
     * stays the same: players set their state far more often than their
     * track.
     */
    class Cache {
        private var key: String? = null
        private var cover: Cover? = null

        @Synchronized
        fun of(packageName: String, metadata: MediaMetadata?): Cover? {
            val bitmap = metadata?.let(::bitmap)
            // By what the track is, not the bitmap's object: each ask of the
            // player brings a new one (N2 review: the cache never hit).
            val key = listOf(
                packageName,
                metadata?.getString(MediaMetadata.METADATA_KEY_TITLE),
                metadata?.getString(MediaMetadata.METADATA_KEY_ARTIST),
                metadata?.getString(MediaMetadata.METADATA_KEY_ALBUM),
                metadata?.getLong(MediaMetadata.METADATA_KEY_DURATION)?.toString(),
                bitmap?.let { "${it.width}x${it.height}" },
            ).joinToString("|")
            if (key != this.key) {
                this.key = key
                cover = bitmap?.let(::encode)
            }
            return cover
        }
    }

    private fun bitmap(metadata: MediaMetadata): Bitmap? =
        listOf(MediaMetadata.METADATA_KEY_ART, MediaMetadata.METADATA_KEY_ALBUM_ART, MediaMetadata.METADATA_KEY_DISPLAY_ICON)
            .firstNotNullOfOrNull { runCatching { metadata.getBitmap(it) }.getOrNull() }

    private fun encode(bitmap: Bitmap): Cover? = runCatching {
        if (bitmap.isRecycled || bitmap.width <= 0 || bitmap.height <= 0) return null
        val (w, h) = fit(bitmap.width, bitmap.height)
        val scaled = if (w == bitmap.width && h == bitmap.height) bitmap else bitmap.scale(w, h)
        val out = ByteArrayOutputStream()
        // A JPEG has no transparency: a cover with some is drawn on black.
        val opaque = if (scaled.config == Bitmap.Config.HARDWARE) scaled.copy(Bitmap.Config.ARGB_8888, false) else scaled
        if (!opaque.compress(Bitmap.CompressFormat.JPEG, QUALITY, out)) return null
        val jpeg = out.toByteArray()
        Cover(id(jpeg), jpeg)
    }.getOrNull()
}
