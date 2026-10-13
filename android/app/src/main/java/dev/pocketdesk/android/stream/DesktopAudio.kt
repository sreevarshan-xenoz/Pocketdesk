package dev.pocketdesk.android.stream

import android.content.Context
import dev.pocketdesk.android.Prefs

/**
 * The app's one [AudioLifecycle]: DecoderHost reports each stream's
 * generation up and down, and the switch calls [setEnabled].
 */
object DesktopAudio {
    private var lifecycle: AudioLifecycle? = null

    @Synchronized
    private fun lifecycle(context: Context): AudioLifecycle {
        lifecycle?.let { return it }
        val app = context.applicationContext
        return AudioLifecycle(Prefs.desktopSound(app)) {
            val player = DesktopAudioPlayer().also { it.start(app) }
            // Off the caller's thread: the switch is on the UI thread, and
            // the player may take a write's worth of time to wind down.
            Stoppable { Thread({ player.stop() }, "desktop-audio-stop").start() }
        }.also { lifecycle = it }
    }

    fun newGeneration(context: Context): Long = lifecycle(context).newGeneration()

    fun streamUp(context: Context, generation: Long) = lifecycle(context).streamUp(generation)

    /** A stream that never got a generation has nothing to stop. */
    fun streamDown(generation: Long) {
        synchronized(this) { lifecycle }?.streamDown(generation)
    }

    fun setEnabled(context: Context, enabled: Boolean) {
        Prefs.setDesktopSound(context, enabled)
        lifecycle(context).setEnabled(enabled)
    }
}
