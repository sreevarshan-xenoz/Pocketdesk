package dev.pocketdesk.android.status

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Ends a reporter's wait between checks early (AP3): [BatteryEffect] reads
 * the phone's state every 15 s, so a ringer turned just after a check
 * reached the computer some 16 s later, past the 15 s §162 promises. A
 * poke while it is busy is kept for its next wait; several are one.
 */
internal class TickWake {
    private val wake = Channel<Unit>(Channel.CONFLATED)

    fun poke() {
        wake.trySend(Unit)
    }

    /** Waits [ms], or until poked; true when poked. */
    suspend fun await(ms: Long): Boolean = withTimeoutOrNull(ms) { wake.receive() } != null
}
