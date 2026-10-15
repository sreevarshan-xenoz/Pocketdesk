package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** When the session's control connection needs dialing, kept apart from Android so it is tested. */
object ControlLink {
    /** How often the session checks its control connection. */
    const val CHECK_MS = 4_000L

    private const val MAX_WAIT_MS = 60_000L

    /**
     * Whether the session must connect: nothing is connected, or the live
     * connection is to another computer.
     */
    fun needsConnect(live: Pair<String, Int>?, address: String, port: Int): Boolean =
        live != (address to port)

    /**
     * How long to wait before the next check after [failures] failed dials
     * in a row: a computer that is asleep or away is not dialed every few
     * seconds for as long as the session screen is open.
     */
    fun nextWaitMs(failures: Int): Long =
        (CHECK_MS shl failures.coerceAtMost(10)).coerceAtMost(MAX_WAIT_MS)
}

/**
 * Keeps the session's control connection to its computer up (R2). It carries
 * what the desktop pushes to the phone (file offers, the HUD, wake details)
 * and the phone's queries. Without it, a dropped network left the session
 * reconnecting its stream while every query went over a throwaway connection
 * and no push arrived until the session ended. It runs while the app is
 * started (picture-in-picture included), not in the background.
 */
@Composable
fun ControlLinkEffect(address: String, controlPort: Int) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val context = LocalContext.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            withContext(Dispatchers.IO) {
                var failures = 0
                while (isActive) {
                    if (ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)) {
                        RustCore.connectToPeer(address, controlPort, HostStore.wanIdentity(context, address))
                        val up = !ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)
                        failures = if (up) 0 else failures + 1
                    } else {
                        failures = 0
                    }
                    delay(ControlLink.nextWaitMs(failures))
                }
            }
        }
    }
}
