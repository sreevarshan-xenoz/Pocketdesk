package dev.pocketdesk.android.clip

import android.content.Context
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore

/**
 * A text sent to a computer's clipboard, from outside a session: on a
 * one-off connection, and, when that fails with no link up, over the
 * internet road with the computer's cached identity (T10a). The Send
 * clipboard tile and Home's clipboard history share it (D3 review). Blocking:
 * call it off the main thread. Whether it went.
 */
object ClipboardSend {
    fun toComputer(ctx: Context, host: HostStore.Host, text: String): Boolean {
        val attempt = { RustCore.sendClipboard(host.address, host.controlPort, text) }
        var sent = attempt()
        if (sent.isFailure && RustCore.controlPeer() == null) {
            HostStore.wanIdentity(ctx, host.address)?.let { wan ->
                RustCore.connectToPeer(host.address, host.controlPort, wan)
                if (RustCore.controlPeer() != null) sent = attempt()
            }
        }
        if (sent.isSuccess) ClipboardShared.lastExchanged = text
        return sent.isSuccess
    }
}
