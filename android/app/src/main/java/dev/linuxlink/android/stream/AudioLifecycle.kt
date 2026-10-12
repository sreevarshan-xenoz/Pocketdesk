package dev.linuxlink.android.stream

/** Something playing that can be told to stop. */
fun interface Stoppable {
    fun stop()
}

/**
 * Which video stream, if any, the desktop's sound plays for (spec A9), and
 * whether the user's switch allows it (A8). Each stream start takes a new
 * generation, and reports from an older one are ignored, so a stop that
 * beats its own start's report cannot leave a player running with no
 * session, and a stale stop cannot silence the next stream. Pure:
 * [DesktopAudio] supplies the player and the pref.
 */
class AudioLifecycle(
    private var enabled: Boolean,
    private val startPlayer: () -> Stoppable,
) {
    private var lastGeneration = 0L

    /** Every generation up to this one has stopped. */
    private var ended = 0L
    private var live: Long? = null
    private var player: Stoppable? = null

    @Synchronized
    fun newGeneration(): Long = ++lastGeneration

    @Synchronized
    fun streamUp(generation: Long) {
        if (generation <= ended) return
        live = generation
        apply()
    }

    @Synchronized
    fun streamDown(generation: Long) {
        ended = maxOf(ended, generation)
        if (live?.let { it <= ended } == true) live = null
        apply()
    }

    @Synchronized
    fun setEnabled(value: Boolean) {
        enabled = value
        apply()
    }

    private fun apply() {
        val wanted = enabled && live != null
        if (wanted && player == null) {
            player = startPlayer()
        } else if (!wanted) {
            player?.stop()
            player = null
        }
    }
}
