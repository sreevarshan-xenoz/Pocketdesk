package dev.pocketdesk.android.clip

import android.app.Activity
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.ui.ClipHistory
import dev.pocketdesk.android.ui.localClipboardText

/** What "Send clipboard" does with what it read (T10b). */
object SendClipboard {
    /** The longest text sent this way; the computer pushes at most 256 KiB back. */
    const val MAX_CHARS = 256 * 1024

    enum class Check { Send, NoComputer, Empty, TooLong }

    fun check(text: String?, hasComputer: Boolean): Check =
        when {
            !hasComputer -> Check.NoComputer
            text.isNullOrBlank() -> Check.Empty
            text.length > MAX_CHARS -> Check.TooLong
            else -> Check.Send
        }
}

/**
 * "Send clipboard" (T10b), from the Quick Settings tile: a window that never
 * shows, because Android lets only the app in focus read the clipboard. It
 * reads the text once it has focus, sends it to the phone's computer over
 * the live link, or opens one (over the internet when it must), says how it
 * went, and closes.
 */
class SendClipboardActivity : Activity() {
    private var done = false

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        val app = applicationContext
        val text = localClipboardText(this)
        val host = PhoneLink.target(app)
        when (SendClipboard.check(text, host != null)) {
            SendClipboard.Check.NoComputer -> say(app, getString(R.string.send_clipboard_no_computer))
            SendClipboard.Check.Empty -> say(app, getString(R.string.send_clipboard_empty))
            SendClipboard.Check.TooLong -> say(app, getString(R.string.send_clipboard_too_long))
            SendClipboard.Check.Send -> Thread { send(app, host!!, text!!) }.start()
        }
        finish()
    }

    private fun send(app: Context, host: HostStore.Host, text: String) {
        val attempt = { RustCore.sendClipboard(host.address, host.controlPort, text) }
        var sent = attempt()
        if (sent.isFailure && RustCore.controlPeer() == null) {
            HostStore.wanIdentity(app, host.address)?.let { wan ->
                RustCore.connectToPeer(host.address, host.controlPort, wan)
                if (RustCore.controlPeer() != null) sent = attempt()
            }
        }
        if (sent.isSuccess) {
            ClipboardShared.lastExchanged = text
            ClipHistory.add(app, text)
            say(app, app.getString(R.string.send_clipboard_sent, host.address))
        } else {
            say(app, app.getString(R.string.send_clipboard_failed, host.address))
        }
    }

    private fun say(app: Context, message: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(app, message, Toast.LENGTH_SHORT).show() }
    }
}
