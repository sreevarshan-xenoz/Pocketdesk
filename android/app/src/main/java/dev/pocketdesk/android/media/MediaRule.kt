package dev.pocketdesk.android.media

/**
 * What the phone tells the computer it plays, and when (T8d). Kept apart
 * from Android so it is tested.
 */
object MediaRule {
    /** The commands a computer may send. */
    val COMMANDS = setOf("playPause", "next", "previous")

    /** One media session, as reported. [packageName] stays on the phone. */
    data class Media(
        val title: String,
        val artist: String,
        val app: String,
        val packageName: String,
        val isPlaying: Boolean,
        val canPlayPause: Boolean,
        val canNext: Boolean,
        val canPrevious: Boolean,
    )

    /** What was last sent: [any] once something has been. */
    data class Sent(val any: Boolean = false, val last: Media? = null)

    /** The session to report: the first playing one, else the most recent (Android lists it first). */
    fun pick(sessions: List<Media>): Media? = sessions.firstOrNull { it.isPlaying } ?: sessions.firstOrNull()

    /** Whether [now] must be sent after [sent]: a change, or `{}` once when playing stops. */
    fun shouldSend(sent: Sent, now: Media?): Boolean = if (sent.any) now != sent.last else now != null

    /** What was sent, as far as a computer [linked] (or not) still knows it. */
    fun forLink(sent: Sent, linked: Boolean): Sent = if (linked) sent else Sent()

    /** Whether a press for [command] is acted on. */
    fun accepts(command: String, settingOn: Boolean): Boolean = settingOn && command in COMMANDS
}
