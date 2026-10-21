package dev.pocketdesk.fixture

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.IBinder
import android.os.SystemClock
import android.util.Log

/**
 * A player for the media checks (N2): a media session with a title, an
 * artist and a cover, playing or paused, that plays no sound. Its presses
 * are written to logcat as `FixturePlayer`.
 * `am start-foreground-service -n dev.pocketdesk.fixture/.PlayerService
 *  [--es title …] [--es artist …] [--ez playing false] [--ei color 0xRRGGBB]`,
 * and `-a dev.pocketdesk.fixture.STOP` to end it.
 */
class PlayerService : Service() {
    private var session: MediaSession? = null
    private var playing = true

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Fixture player", NotificationManager.IMPORTANCE_LOW))
        val session = session ?: MediaSession(this, "FixturePlayer").also {
            it.setCallback(object : MediaSession.Callback() {
                override fun onPlay() = press("play") { playing = true }
                override fun onPause() = press("pause") { playing = false }
                override fun onSkipToNext() = press("next") {}
                override fun onSkipToPrevious() = press("previous") {}
            })
            session = it
        }
        val notification = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle("Fixture player")
            .setStyle(Notification.MediaStyle().setMediaSession(session.sessionToken))
            .build()
        if (android.os.Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        } else {
            startForeground(NOTIFICATION, notification)
        }
        if (intent?.action == ACTION_STOP) {
            session.release()
            this.session = null
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            Log.i(TAG, "stopped")
            return START_NOT_STICKY
        }
        playing = intent?.getBooleanExtra("playing", true) ?: true
        session.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, intent?.getStringExtra("title") ?: "Fixture song")
                .putString(MediaMetadata.METADATA_KEY_ARTIST, intent?.getStringExtra("artist") ?: "Pocketdesk")
                .putString(MediaMetadata.METADATA_KEY_ALBUM, "Fixture")
                .putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, cover(intent?.getIntExtra("color", 0x2E7D6B) ?: 0x2E7D6B))
                .build(),
        )
        session.isActive = true
        state()
        Log.i(TAG, "playing=$playing")
        return START_NOT_STICKY
    }

    private fun press(command: String, then: () -> Unit) {
        Log.i(TAG, command)
        then()
        state()
    }

    private fun state() {
        session?.setPlaybackState(
            PlaybackState.Builder()
                .setActions(
                    PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE or
                        PlaybackState.ACTION_SKIP_TO_NEXT or PlaybackState.ACTION_SKIP_TO_PREVIOUS,
                )
                .setState(
                    if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                    0,
                    1f,
                    SystemClock.elapsedRealtime(),
                )
                .build(),
        )
    }

    /** A 400 px cover, larger than the 256 px the phone sends, in [color]. */
    private fun cover(color: Int): Bitmap {
        val bitmap = Bitmap.createBitmap(400, 400, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(Color.rgb(color shr 16 and 0xFF, color shr 8 and 0xFF, color and 0xFF))
        canvas.drawCircle(200f, 200f, 120f, Paint().apply { this.color = Color.WHITE })
        return bitmap
    }

    override fun onDestroy() {
        session?.release()
        session = null
        super.onDestroy()
    }

    private companion object {
        const val TAG = "FixturePlayer"
        const val CHANNEL = "fixture-player"
        const val NOTIFICATION = 2
        const val ACTION_STOP = "dev.pocketdesk.fixture.STOP"
    }
}
