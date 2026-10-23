package dev.pocketdesk.android.ring

/**
 * How a ring from the computer shakes and flashes while it sounds (P11):
 * a vibration pattern that repeats, and a torch that blinks on and off.
 */
object RingPattern {
    /** Wait, buzz, pause, buzz, rest: repeated from its start. */
    val VIBRATION: LongArray = longArrayOf(0, 800, 300, 800, 1200)

    /** The torch's half beat: on this long, then off as long. */
    const val TORCH_STEP_MS = 400L

    /** Whether the torch is on [elapsedMs] into a ring. */
    fun torchOn(elapsedMs: Long): Boolean = elapsedMs >= 0 && (elapsedMs / TORCH_STEP_MS) % 2 == 0L

    /** The torch's next change after [elapsedMs]: when to look again. */
    fun nextTorchChangeMs(elapsedMs: Long): Long =
        TORCH_STEP_MS - (elapsedMs.coerceAtLeast(0) % TORCH_STEP_MS)
}
