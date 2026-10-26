package dev.pocketdesk.android.pair

import android.content.Context
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore

/**
 * A session's computer pinned on first use, forgotten when the session ends
 * with no card saved there (BC3): a found computer is saved only once it
 * pairs, and one that never paired left its pin with no card to remove it,
 * so another computer at that address could not be paired short of
 * clearing the app's data.
 */
object SessionPins {
    /**
     * How long after a session ends its pin is looked at (review: its own
     * teardown, still running, could otherwise pin it again on first use).
     */
    const val SETTLE_MS = 3_000L

    /** The address of the session on screen, if any. */
    @Volatile
    var current: String? = null

    /**
     * Whether the pin for [address] goes, with [saved] the saved addresses
     * and [now] the session on screen (review: one opened again to the same
     * computer keeps it).
     */
    fun forgets(address: String, saved: List<String>, now: String?): Boolean =
        address.isNotEmpty() && address !in saved && address != now

    /** Once the session at [address] has ended, forget its pin when no card is saved there. */
    fun afterSession(context: Context, address: String) {
        Thread {
            Thread.sleep(SETTLE_MS)
            runCatching {
                if (forgets(address, HostStore.hosts(context).map { it.address }, current)) {
                    RustCore.forgetPeerCertificate(address)
                }
            }
        }.start()
    }
}
