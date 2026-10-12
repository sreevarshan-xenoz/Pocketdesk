package dev.linuxlink.android.stream

import android.media.AudioManager

/**
 * When the desktop's sound may be heard (spec A7, revised after the final
 * review). Focus is asked for only once the desktop actually sends sound,
 * so connecting to a silent desktop does not pause the phone's music. A
 * delayed grant (asked during a call) waits for its gain, and a transient
 * loss resumes. A permanent loss is the user choosing another app, so it
 * is not taken back: turning "Desktop sound" off and on starts a new
 * player that may ask again. The focus listener runs on another thread,
 * so every method is synchronized.
 */
class FocusPolicy {
    private var state = IDLE

    /** Whether to ask for focus now that there is sound to play. */
    @Synchronized
    fun wantsRequest(): Boolean = state == IDLE

    @Synchronized
    fun onRequestResult(result: Int) {
        state = when (result) {
            AudioManager.AUDIOFOCUS_REQUEST_GRANTED -> HELD
            AudioManager.AUDIOFOCUS_REQUEST_DELAYED -> PAUSED
            else -> LOST
        }
    }

    @Synchronized
    fun onChange(change: Int) {
        state = when (change) {
            AudioManager.AUDIOFOCUS_GAIN -> HELD
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> PAUSED
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> DUCKED
            AudioManager.AUDIOFOCUS_LOSS -> LOST
            else -> state
        }
    }

    @Synchronized
    fun audible(): Boolean = state == HELD || state == DUCKED

    @Synchronized
    fun volume(): Float = if (state == DUCKED) DUCKED_VOLUME else 1f

    private companion object {
        const val IDLE = 0
        const val HELD = 1
        const val DUCKED = 2
        const val PAUSED = 3
        const val LOST = 4
        const val DUCKED_VOLUME = 0.3f
    }
}
