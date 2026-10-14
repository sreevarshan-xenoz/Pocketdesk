package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** When the session's control connection needs dialing, kept apart from Android so it is tested. */
object ControlLink {
    /** How often the session checks its control connection. */
    const val CHECK_MS = 4_000L

    /**
     * Whether the session must connect: nothing is connected, or the live
     * connection is to another computer.
     */
    fun needsConnect(live: Pair<String, Int>?, address: String, port: Int): Boolean =
        live != (address to port)
}

/**
 * Keeps the session's control connection to its computer up (R2). It carries
 * what the desktop pushes to the phone (file offers, the HUD, wake details)
 * and the phone's queries. Without it, a dropped network left the session
 * reconnecting its stream while every query went over a throwaway connection
 * and no push arrived until the session ended.
 */
@Composable
fun ControlLinkEffect(address: String, controlPort: Int) {
    LaunchedEffect(address, controlPort) {
        withContext(Dispatchers.IO) {
            while (isActive) {
                if (ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)) {
                    RustCore.connectToPeer(address, controlPort)
                }
                delay(ControlLink.CHECK_MS)
            }
        }
    }
}
