package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.isActive
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * Two-way clipboard sync over the KDE Connect control channel (Tier 1 #4).
 *
 * Push: local clipboard changes go out via sendClipboard. Pull: the server
 * never broadcasts clipboard changes on its own, so the remote value is
 * polled with getClipboard. lastLocal/lastRemote tracking prevents the
 * classic echo loop (our push arriving as a "remote change" and bouncing
 * back). Runs only while [enabled]; the control connection is (re)established
 * whenever a send fails.
 */
@Composable
fun ClipboardSyncEffect(
    address: String,
    controlPort: Int,
    enabled: Boolean,
) {
    val context = LocalContext.current
    LaunchedEffect(enabled, address, controlPort) {
        if (!enabled) return@LaunchedEffect
        withContext(Dispatchers.IO) {
            RustCore.connectToPeer(address, controlPort)
            var lastLocal = localClipboardText(context)
            var lastRemote: String? = null
            lastLocal?.let {
                if (RustCore.sendClipboard(address, controlPort, it).isSuccess) lastRemote = it
            }
            while (isActive) {
                delay(4000)

                val local = localClipboardText(context)
                if (local != null && local != lastLocal) {
                    lastLocal = local
                    if (RustCore.sendClipboard(address, controlPort, local).isSuccess) {
                        ClipHistory.add(context, local)
                        lastRemote = local
                    } else {
                        RustCore.connectToPeer(address, controlPort)
                    }
                }

                val remote = RustCore.getClipboard(address, controlPort).getOrNull()
                if (remote != null && remote != lastRemote) {
                    lastRemote = remote
                    ClipHistory.add(context, remote)
                    if (remote != lastLocal) {
                        writeLocalClipboard(context, remote)
                        lastLocal = remote
                    }
                }
            }
        }
    }
}