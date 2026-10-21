package dev.pocketdesk.android.media

/**
 * What the phone shows of the computer's music in its media controls (N5),
 * and when it asks the computer to tell it. Kept apart from Android so it
 * is tested.
 */
object ComputerMediaRule {
    /** What the computer plays, as its push says (T8c's `playing`). */
    data class Playing(
        val title: String,
        val artist: String,
        val app: String,
        val isPlaying: Boolean,
        val canPlayPause: Boolean,
        val canNext: Boolean,
        val canPrevious: Boolean,
        /** Its cover's id, 16 hex digits; null for none. */
        val artId: String? = null,
    )

    /** No push this long: the link dropped, and the session ends. */
    const val SILENT_MS = 150_000L

    /** A paused player shows this long, then the session ends. */
    const val PAUSED_MS = 5 * 60_000L

    /** The commands a press sends, as T8c's `mediaControl` takes them. */
    const val PLAY_PAUSE = "playPause"
    const val NEXT = "next"
    const val PREVIOUS = "previous"

    /** Whether a push from [host]:[port] is the own computer's, the only one shown. */
    fun fromOwn(host: String, port: Int, ownAddress: String?, ownPort: Int?): Boolean =
        ownAddress != null && host == ownAddress && port == ownPort

    /** A cover's id as the computer names one, or null. */
    fun artId(id: String?): String? = id?.takeIf { it.length == 16 && it.all { c -> c in '0'..'9' || c in 'a'..'f' } }

    /** What shows: the last push, until it ages out. */
    class Shown {
        private var playing: Playing? = null
        private var heardMs = 0L
        private var pausedSinceMs: Long? = null

        /** A push said [now] (null: nothing plays) at [nowMs]. */
        fun heard(now: Playing?, nowMs: Long) {
            heardMs = nowMs
            pausedSinceMs = when {
                now == null || now.isPlaying -> null
                // Still paused: since it first was.
                playing?.isPlaying == false -> pausedSinceMs ?: nowMs
                else -> nowMs
            }
            playing = now
        }

        /** What shows at [nowMs], or null once nothing plays, the computer went quiet or a pause ran long. */
        fun current(nowMs: Long): Playing? {
            val now = playing ?: return null
            if (nowMs - heardMs >= SILENT_MS) return null
            val since = pausedSinceMs
            if (!now.isPlaying && since != null && nowMs - since >= PAUSED_MS) return null
            return now
        }

        /** Forget it all: the switch went off, or the own computer changed. */
        fun clear() {
            playing = null
            pausedSinceMs = null
        }
    }

    /** A watch not answered by a push this long is asked again: the computer pushes each minute. */
    const val QUIET_MS = 75_000L

    /** At most this often is a watch asked again on one link. */
    const val REASK_MS = 30_000L

    /**
     * Whether to ask the computer to tell, or to stop, on each link: a
     * computer forgets a phone that unlinks, so each new link is asked; and
     * again when no push came for [QUIET_MS] or a cover is missing, since
     * a link that dropped and came back between two looks seems the same.
     */
    class Watch {
        private var told: Pair<Any, Boolean>? = null
        private var askedMs = 0L

        /**
         * What to send on [link] (null: no link) while [wanted] at [nowMs]:
         * `true`, `false` or nothing. The computer seems not to know this
         * phone watches when no push came since [heardMs], or when
         * [coverMissing]: a cover named that the phone does not hold.
         */
        fun due(link: Any?, wanted: Boolean, nowMs: Long = 0, heardMs: Long? = null, coverMissing: Boolean = false): Boolean? {
            if (link == null) {
                told = null
                return null
            }
            val last = told
            val sameLink = last != null && last.first == link
            return when {
                sameLink && last!!.second == wanted -> {
                    val quiet = nowMs - maxOf(askedMs, heardMs ?: Long.MIN_VALUE) >= QUIET_MS
                    (wanted && (quiet || coverMissing) && nowMs - askedMs >= REASK_MS).takeIf { it }
                }
                // Never asked on this link: nothing to stop.
                !wanted && !sameLink -> null
                else -> wanted
            }
        }

        /** [on] was sent on [link] at [nowMs]. */
        fun sent(link: Any, on: Boolean, nowMs: Long = 0) {
            told = link to on
            askedMs = nowMs
        }
    }
}
