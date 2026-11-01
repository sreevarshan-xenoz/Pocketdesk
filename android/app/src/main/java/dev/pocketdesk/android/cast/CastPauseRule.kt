package dev.pocketdesk.android.cast

/**
 * Hands-free: when the computer a phone's screen is shown to goes away, the
 * cast may be paused instead of ended, so that the same computer's next
 * "open this app" needs no new consent from Android. Pure, for JVM tests;
 * [CastService] does it.
 *
 * It is a hands-free feature: it needs "Let the computer open my apps" and
 * "Accept screen sharing for my computers" both on. What is kept is only the owner's earlier consent: the projection and its
 * virtual display, with no encoder and no surface, so nothing is captured.
 * It lasts [PAUSE_MS] at most, its notification stays up the whole time, and
 * it resumes for the one computer it was showing, never for another.
 */
object CastPauseRule {
    /** How long a paused cast is kept: ten minutes. */
    const val PAUSE_MS = 10 * 60_000L

    /** What the session says when the computer ended it. */
    const val SESSION_ENDED = "session ended"

    /**
     * Whether a cast that ended is paused instead. Only a screen shown (not
     * the camera, not sound alone) that did reach the computer, while the
     * phone's owner lets the computer open apps and that computer is the
     * phone's own, and only when it is the computer that left or the link
     * that dropped ([closed] null or [SESSION_ENDED]): a refusal, a changed
     * permission, an unpaired computer or another phone's camera ends it
     * for good, and so does the owner's own Stop ([stoppedByOwner]) or
     * Android ending the projection.
     */
    fun pauses(
        screen: Boolean,
        shown: Boolean,
        appsOn: Boolean,
        optIn: Boolean,
        ownComputer: Boolean,
        closed: String?,
        stoppedByOwner: Boolean = false,
    ): Boolean = screen && shown && kept(appsOn, optIn, ownComputer) && !stoppedByOwner &&
        (closed == null || closed == SESSION_ENDED)

    /** How often a paused share looks at whether it may still be kept. */
    const val WATCH_MS = 2_000L

    /**
     * Whether a pause may be kept at all, and goes on being: "Let the
     * computer open my apps" ([appsOn]) and "Accept screen sharing for my
     * computers" ([optIn]) are both still on and the computer is still the
     * phone's own and paired ([ownComputer]). Looked at every [WATCH_MS]: the
     * moment any turns off, the share is given back.
     */
    fun kept(appsOn: Boolean, optIn: Boolean, ownComputer: Boolean): Boolean =
        appsOn && optIn && ownComputer

    /** Whether a pause begun at [pausedAt] has run out at [now] (both a monotonic clock). */
    fun expired(pausedAt: Long, now: Long): Boolean = now - pausedAt >= PAUSE_MS

    /**
     * Whether a paused cast serves an ask from [asking]: it was paused for
     * that very computer ([pausedFor]), it has not run out, the switch is
     * still on and the computer is still the phone's own.
     */
    fun mayResume(
        pausedFor: String?,
        asking: String,
        pausedAt: Long,
        now: Long,
        appsOn: Boolean,
        optIn: Boolean,
        ownComputer: Boolean,
    ): Boolean = pausedFor != null && pausedFor == asking && !expired(pausedAt, now) &&
        kept(appsOn, optIn, ownComputer)

    /** The whole minutes a pause has left at [now], at least one while it lasts. */
    fun minutesLeft(pausedAt: Long, now: Long): Int =
        (((PAUSE_MS - (now - pausedAt)).coerceAtLeast(0) + 59_999L) / 60_000L).toInt()
}
