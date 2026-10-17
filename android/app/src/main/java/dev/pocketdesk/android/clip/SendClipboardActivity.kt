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

    enum class Check { Send, NoComputer, Empty, TooLong, AskPrivate }

    /**
     * What to do with [text]; one the app it came from marked [private] (a
     * password manager's copy) is sent only once the person says so (N2).
     */
    fun check(text: String?, hasComputer: Boolean, private: Boolean = false): Check =
        when {
            !hasComputer -> Check.NoComputer
            text.isNullOrBlank() -> Check.Empty
            text.length > MAX_CHARS -> Check.TooLong
            private -> Check.AskPrivate
            else -> Check.Send
        }

    /** Whether the copy on the clipboard is marked sensitive by the app that made it. */
    fun clipIsPrivate(ctx: Context): Boolean =
        ctx.getSystemService(android.content.ClipboardManager::class.java)
            ?.primaryClip
            ?.description
            ?.extras
            // ClipDescription.EXTRA_IS_SENSITIVE, by value: the constant is API 33.
            ?.getBoolean("android.content.extra.IS_SENSITIVE") == true
}

/**
 * "Send clipboard" (T10b), from the Quick Settings tile: a window that never
 * shows, because Android lets only the app in focus read the clipboard. It
 * reads the text once it has focus, sends it to the phone's computer over
 * the live link, or opens one (over the internet when it must), says how it
 * went, and closes once the send is done.
 */
class SendClipboardActivity : Activity() {
    private var done = false

    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        super.onCreate(savedInstanceState)
        // The window stays while the send runs (a closed one could leave the
        // process frozen mid-send), so it must let every touch through.
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        val app = applicationContext
        val text = localClipboardText(this)
        val host = PhoneLink.target(app)
        when (SendClipboard.check(text, host != null, SendClipboard.clipIsPrivate(this))) {
            SendClipboard.Check.NoComputer -> say(app, getString(R.string.send_clipboard_no_computer))
            SendClipboard.Check.Empty -> say(app, getString(R.string.send_clipboard_empty))
            SendClipboard.Check.TooLong -> say(app, getString(R.string.send_clipboard_too_long))
            SendClipboard.Check.Send -> {
                Thread {
                    send(app, host!!, text!!)
                    runOnUiThread { finish() }
                }.start()
                return
            }
            SendClipboard.Check.AskPrivate -> {
                askPrivate(app, host!!, text!!)
                return
            }
        }
        finish()
    }

    /** A private copy: sent only when the person says so (N2). */
    private fun askPrivate(app: Context, host: HostStore.Host, text: String) {
        // The window takes touches for the question, and lets them through again after.
        window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE)
        android.app.AlertDialog.Builder(this)
            .setTitle(R.string.send_clipboard_private_title)
            .setMessage(getString(R.string.send_clipboard_private_text, HostStore.shownName(app, host.address)))
            .setPositiveButton(R.string.send_clipboard_private_send) { _, _ ->
                window.addFlags(android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE)
                Thread {
                    send(app, host, text, private = true)
                    runOnUiThread { finish() }
                }.start()
            }
            .setNegativeButton(R.string.cancel) { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }

    /** Send [text]; a [private] copy is not kept in the clipboard history (N2 review). */
    private fun send(app: Context, host: HostStore.Host, text: String, private: Boolean = false) {
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
            if (!private) ClipHistory.add(app, text)
            say(app, app.getString(R.string.send_clipboard_sent, HostStore.shownName(app, host.address)))
        } else {
            say(app, app.getString(R.string.send_clipboard_failed, HostStore.shownName(app, host.address)))
        }
    }

    private fun say(app: Context, message: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(app, message, Toast.LENGTH_SHORT).show() }
    }
}
