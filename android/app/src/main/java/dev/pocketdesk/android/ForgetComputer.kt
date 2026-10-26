package dev.pocketdesk.android

import android.content.Context
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.AlsoToPrefs
import dev.pocketdesk.android.shortcuts.DeckWidget
import dev.pocketdesk.android.shortcuts.DeckWidgetPlan
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * A saved computer removed from the phone, with what it kept for it: one
 * way for the card's Remove and the doctor's Pair again (BC1: Pair again
 * left its link, its deck list and its "also send to" pick).
 */
object ForgetComputer {
    /**
     * Forget the computer at [address]. The returned job closes its link and
     * forgets its key; a new pairing code for the address is trusted once it
     * is done.
     */
    fun forget(context: Context, scope: CoroutineScope, address: String): Job {
        // Its "also send notifications to" pick goes with it (AX3).
        val removedId = HostStore.pairedDesktopId(context, address)
        HostStore.remove(context, address)
        // The deck widget's list for it goes too (AZ2 review).
        DeckWidget.forgetRemoved(context, DeckWidgetPlan.computerKey(address, removedId))
        AlsoToPrefs.forgetRemoved(
            context,
            removedId,
            HostStore.hosts(context).mapNotNull { HostStore.pairedDesktopId(context, it.address) }.toSet(),
        )
        // Close a link to it (AX1): the next own computer links then. Then
        // forget its certificate, so a new pairing code for this address can
        // be trusted again, and what it said of itself (AZ3), after the
        // link, so a handshake on it cannot say it again (review).
        return scope.launch(Dispatchers.IO) {
            RustCore.closeControlLinkTo(address)
            RustCore.forgetPeerCertificate(address)
        }
    }
}
