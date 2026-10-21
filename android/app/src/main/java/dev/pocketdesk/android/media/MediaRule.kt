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
        /** Its cover's id (N2), 16 hex digits of the JPEG's SHA-256; null for none. */
        val artId: String? = null,
    )

    /**
     * What plays is said again this often even unchanged: a computer that
     * dropped it (Media turned off, then on) hears it again.
     */
    const val RESEND_MS = 60_000L

    /** What was last sent ([any] once something has been), and when ([atMs]). */
    data class Sent(val any: Boolean = false, val last: Media? = null, val atMs: Long = 0)

    /** The session to report: the first playing one, else the most recent (Android lists it first). */
    fun pick(sessions: List<Media>): Media? = sessions.firstOrNull { it.isPlaying } ?: sessions.firstOrNull()

    /**
     * Whether [now] must be sent at [nowMs] after [sent]: a change, `{}`
     * once when playing stops, or what plays again every [RESEND_MS].
     */
    fun shouldSend(sent: Sent, now: Media?, nowMs: Long = 0): Boolean = when {
        !sent.any -> now != null
        now != sent.last -> true
        else -> now != null && nowMs - sent.atMs >= RESEND_MS
    }

    /** What was sent, as far as a computer [linked] (or not) still knows it. */
    fun forLink(sent: Sent, linked: Boolean): Sent = if (linked) sent else Sent()

    /**
     * How long a paused player is said again by the background reporter
     * (N2): the computer's media controls keep a paused phone this long (K7).
     */
    const val PAUSED_MS = 5 * 60_000L

    /** Whether [now] is current at [nowMs]: playing, or paused since [pausedSinceMs] for under [PAUSED_MS]. */
    fun current(now: Media?, nowMs: Long, pausedSinceMs: Long?): Boolean = when {
        now == null -> false
        now.isPlaying -> true
        else -> pausedSinceMs != null && nowMs - pausedSinceMs < PAUSED_MS
    }

    /**
     * Whether the background reporter (N2) sends [now] at [nowMs] after
     * [sent]: a change, `{}` once when playing stops, or what is
     * [current] again every [RESEND_MS]; a player paused past [PAUSED_MS]
     * is left to age out on the computer.
     */
    fun backgroundDue(sent: Sent, now: Media?, nowMs: Long, pausedSinceMs: Long?): Boolean = when {
        !sent.any -> now != null
        now != sent.last -> true
        else -> current(now, nowMs, pausedSinceMs) && nowMs - sent.atMs >= RESEND_MS
    }

    /** Whether the cover [artId] goes with a report: one the link has not had ([sentArt]). */
    fun sendArt(artId: String?, sentArt: Set<String>): Boolean = artId != null && artId !in sentArt

    /** Whether a press for [command] is acted on. */
    fun accepts(command: String, settingOn: Boolean): Boolean = settingOn && command in COMMANDS
}
