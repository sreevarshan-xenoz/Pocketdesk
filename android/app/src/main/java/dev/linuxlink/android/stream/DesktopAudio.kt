package dev.linuxlink.android.stream

import android.content.Context
import dev.linuxlink.android.Prefs

/**
 * Plays the desktop's sound exactly while a video stream is up and the
 * user's switch is on (spec A8/A9). The stream reports up/down from
 * DecoderHost; the switch calls [setEnabled].
 */
object DesktopAudio {
    private var appContext: Context? = null
    private var streamUp = false
    private var player: DesktopAudioPlayer? = null

    @Synchronized
    fun streamUp(context: Context) {
        appContext = context.applicationContext
        streamUp = true
        apply()
    }

    @Synchronized
    fun streamDown() {
        streamUp = false
        apply()
    }

    @Synchronized
    fun setEnabled(context: Context, enabled: Boolean) {
        appContext = context.applicationContext
        Prefs.setDesktopSound(context, enabled)
        apply()
    }

    private fun apply() {
        val context = appContext
        val wanted = streamUp && context != null && Prefs.desktopSound(context)
        if (wanted && player == null) {
            player = DesktopAudioPlayer().also { it.start(context!!) }
        } else if (!wanted) {
            val stopping = player ?: return
            player = null
            // Off the caller's thread: the switch is on the UI thread, and
            // the player may take a write's worth of time to wind down.
            Thread({ stopping.stop() }, "desktop-audio-stop").start()
        }
    }
}
