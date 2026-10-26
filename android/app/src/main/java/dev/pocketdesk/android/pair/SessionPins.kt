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
    /** Whether the pin for [address] goes, with [saved] the saved addresses. */
    fun forgets(address: String, saved: List<String>): Boolean = address.isNotEmpty() && address !in saved

    /** Forget [address]'s pin when no card is saved there. Blocking: off the main thread. */
    fun afterSession(context: Context, address: String) {
        if (forgets(address, HostStore.hosts(context).map { it.address })) RustCore.forgetPeerCertificate(address)
    }
}
