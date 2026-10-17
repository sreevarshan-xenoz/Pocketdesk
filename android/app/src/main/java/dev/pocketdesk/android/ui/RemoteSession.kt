package dev.pocketdesk.android.ui

import dev.pocketdesk.android.R

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
            // First: the phone closes a session it finds too old, so a
            // closed reason follows it at once and must not hide it.
            remote == "tooOld" -> State.TooOld
            closedReason != null -> State.Down
            remote == "ready" -> State.Ready
            remote == "waiting" -> if (current == State.Ready) State.Ready else State.Waiting
            // No session any more: it went away under us.
            current == State.Ready || current == State.Waiting -> State.Down
            else -> current
        }

    /**
     * A key on the remote's rows: its label, the Android key code the bridge
     * maps, what a screen reader says for a symbol label (0: the label), and
     * the label in the phone's language where keyboards name the key
     * differently (U6; 0: [label]).
     */
    data class Key(val label: String, val keyCode: Int, val spoken: Int = 0, val labelRes: Int = 0)

    /** Media: Previous, Play/Pause, Next, and the volume. */
    val media =
        listOf(
            Key("⏮", 88, R.string.remote_key_previous), // KEYCODE_MEDIA_PREVIOUS
            Key("⏯", 85, R.string.remote_key_play_pause), // KEYCODE_MEDIA_PLAY_PAUSE
            Key("⏭", 87, R.string.remote_key_next), // KEYCODE_MEDIA_NEXT
            Key("Vol −", 25, R.string.remote_key_volume_down, R.string.key_volume_down), // KEYCODE_VOLUME_DOWN
            Key("Vol +", 24, R.string.remote_key_volume_up, R.string.key_volume_up), // KEYCODE_VOLUME_UP
        )

    /** A presenter's keys: slides back and on, start the show, end it, blank the screen. */
    val presenter =
        listOf(
            Key("PgUp", 92, R.string.remote_key_page_up, R.string.key_page_up), // KEYCODE_PAGE_UP
            Key("PgDn", 93, R.string.remote_key_page_down, R.string.key_page_down), // KEYCODE_PAGE_DOWN
            Key("◀", 21, R.string.remote_key_left), // KEYCODE_DPAD_LEFT
            Key("▶", 22, R.string.remote_key_right), // KEYCODE_DPAD_RIGHT
            Key("F5", 135), // KEYCODE_F5
            Key("Esc", 111, labelRes = R.string.key_escape), // KEYCODE_ESCAPE
            Key("B", 30), // KEYCODE_B
        )
}
