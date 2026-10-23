package dev.pocketdesk.android.ring

/**
 * The alarm volume a ring raised (N1), put back.
 *
 * Never below the least Android lets this app set (`getStreamMinVolume`, 6
 * for the alarm on Android 14 without a privileged permission): a saved 1
 * could never be put back, and the listener retried it every tick for good
 * (S2, on the OPPO).
 */
object AlarmRestore {
    /** Failed puts-back a saved volume gets before it is dropped. */
    const val MAX_TRIES = 5

    /**
     * What to set it to, or null: leave it and forget the saved volume. The
     * ring's own end always puts it back, a volume key pressed during it
     * included; after the ring's process died ([afterDeath]) only while the
     * ring's raise still stands, since a volume changed since is the owner's.
     */
    fun target(before: Int, now: Int, max: Int, min: Int, afterDeath: Boolean): Int? =
        if (afterDeath && now != max) null else maxOf(before, min)

    /** Whether a saved volume is dropped after [tries] failed puts-back. */
    fun giveUp(tries: Int): Boolean = tries >= MAX_TRIES

    /**
     * Whether a new ring keeps the volume an earlier ring saved: only while
     * that ring's raise still stands; otherwise the owner set one since.
     */
    fun keepsSaved(now: Int, max: Int): Boolean = now == max
}
