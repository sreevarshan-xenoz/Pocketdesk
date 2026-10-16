package dev.pocketdesk.android.ui

/**
 * The phone as a remote (T10i): the state of an input-only session, as the
 * bridge reports it, and the keys its rows send. Pure, for JVM tests.
 */
object RemoteSession {
    enum class State {
        /** Dialing the computer. */
        Connecting,

        /** Connected; the computer has not admitted it yet (its owner may be asked). */
        Waiting,

        /** Admitted: input goes to the computer. */
        Ready,

        /** The computer streamed to it anyway: its Pocketdesk is too old for a remote. */
        TooOld,

        /** It ended, or never came up. */
        Down,
    }

    private val STATE = Regex("\"state\"\\s*:\\s*\"(\\w+)\"")

    /** The bridge's `{"state": ...}`: none, waiting, ready or tooOld. */
    fun parse(json: String): String = STATE.find(json)?.groupValues?.get(1) ?: "none"

    /**
     * The next state, from the current one, the bridge's [remote] state and
     * the reason the computer closed the session ([closedReason], null while
     * open). Admitted stays admitted until the session ends.
     */
    fun next(current: State, remote: String, closedReason: String?): State =
        when {
            closedReason != null -> State.Down
            remote == "tooOld" -> State.TooOld
            remote == "ready" -> State.Ready
            remote == "waiting" -> if (current == State.Ready) State.Ready else State.Waiting
            // No session any more: it went away under us.
            current == State.Ready || current == State.Waiting -> State.Down
            else -> current
        }

    /** A key on the remote's rows: its label and the Android key code the bridge maps. */
    data class Key(val label: String, val keyCode: Int)

    /** Media: Previous, Play/Pause, Next, and the volume. */
    val media =
        listOf(
            Key("⏮", 88), // KEYCODE_MEDIA_PREVIOUS
            Key("⏯", 85), // KEYCODE_MEDIA_PLAY_PAUSE
            Key("⏭", 87), // KEYCODE_MEDIA_NEXT
            Key("Vol −", 25), // KEYCODE_VOLUME_DOWN
            Key("Vol +", 24), // KEYCODE_VOLUME_UP
        )

    /** A presenter's keys: slides back and on, start the show, end it, blank the screen. */
    val presenter =
        listOf(
            Key("PgUp", 92), // KEYCODE_PAGE_UP
            Key("PgDn", 93), // KEYCODE_PAGE_DOWN
            Key("◀", 21), // KEYCODE_DPAD_LEFT
            Key("▶", 22), // KEYCODE_DPAD_RIGHT
            Key("F5", 135), // KEYCODE_F5
            Key("Esc", 111), // KEYCODE_ESCAPE
            Key("B", 30), // KEYCODE_B
        )
}
