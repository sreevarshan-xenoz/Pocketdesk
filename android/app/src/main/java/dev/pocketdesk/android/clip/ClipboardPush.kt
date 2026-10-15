package dev.pocketdesk.android.clip

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.ui.ClipHistory
import dev.pocketdesk.android.ui.writeLocalClipboard

/**
 * The text that last crossed between this phone and its computer, either
 * way (T10b). The session's clipboard sync, the background applier and
 * "Send clipboard" all note it, so none of them moves the same text back.
 */
object ClipboardShared {
    @Volatile
    var lastExchanged: String? = null
}

/** The choices of the background applier (T10b). */
object ClipboardPushRule {
    /** Put [incoming] on the phone's clipboard? Not blank, and not what just crossed. */
    fun shouldApply(incoming: String, lastExchanged: String?): Boolean =
        incoming.isNotBlank() && incoming != lastExchanged

    /** Text is taken only from the computer the phone sends to. */
    fun fromOwnComputer(host: String, own: String?): Boolean = own != null && host == own
}

/**
 * The computer's clipboard on the phone without a session (T10b): run on
 * the notification listener's 5 s tick, it keeps the control link to the
 * phone's computer up (over the internet when it must) and puts the text
 * the computer pushed on the phone's clipboard. Android lets a background
 * app write the clipboard, not read it.
 */
class BackgroundClipboard(private val ctx: Context) {
    private var lastConnectMs = 0L
    private val main = Handler(Looper.getMainLooper())

    fun tick() {
        if (!Prefs.clipboardInBackground(ctx)) return
        val own = PhoneLink.target(ctx) ?: return
        val now = SystemClock.elapsedRealtime()
        if (RustCore.controlPeer() == null && now - lastConnectMs >= RECONNECT_MS) {
            lastConnectMs = now
            RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
        }
        val pushed = RustCore.takeClipboardPush() ?: return
        if (!ClipboardPushRule.fromOwnComputer(pushed.host, own.address)) return
        if (!ClipboardPushRule.shouldApply(pushed.content, ClipboardShared.lastExchanged)) return
        ClipboardShared.lastExchanged = pushed.content
        main.post {
            writeLocalClipboard(ctx, pushed.content)
            ClipHistory.add(ctx, pushed.content)
        }
    }

    private companion object {
        const val RECONNECT_MS = 60_000L
    }
}
