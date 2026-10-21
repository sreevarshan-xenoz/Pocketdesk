package dev.pocketdesk.android.clip

import android.app.Activity
import android.util.Log
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.ui.ClipHistory
import dev.pocketdesk.android.ui.localClipboardText

/**
 * A copy just made on the phone, read and sent by itself (M10): a window
 * that never shows, started by C8's service when SystemUI's clipboard
 * overlay comes up, since Android lets only the app in front read the
 * clipboard. It reads the text once it has focus, closes at once so the
 * app the copy came from has its focus back, and sends it to the phone's
 * own computer as the tile does, saying nothing. A copy marked sensitive,
 * or the text that just crossed, is never sent.
 */
class CopyReadActivity : Activity() {
    private var done = false

    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE)
        // Focus that never comes (the screen off, split screen) must not
        // leave it over the app (M10 review).
        window.decorView.postDelayed({ if (!done) close() }, DEADLINE_MS)
    }

    /** Close at once, with no animation. */
    private fun close() {
        done = true
        finish()
        if (android.os.Build.VERSION.SDK_INT >= 34) {
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        val app = applicationContext
        val text = localClipboardText(this)
        val private = SendClipboard.clipIsPrivate(this)
        val host = PhoneLink.target(app)
        close()
        if (host == null || !CopyWatchRule.send(text, private, ClipboardShared.lastExchanged, CopyMemory.lastRead)) {
            Log.i(TAG, "A copy not sent (${if (private) "sensitive" else "nothing new"})")
            return
        }
        // Noted before the send, so a read while it is on its way does not
        // send it again (M10 review).
        CopyMemory.lastRead = text
        // The process stays: the notification listener and this service are bound.
        Thread({
            if (ClipboardSend.toComputer(app, host, text!!)) {
                ClipHistory.add(app, text)
                Log.i(TAG, "A copy sent by itself")
            } else {
                Log.i(TAG, "A copy could not be sent")
            }
        }, "copy-send").start()
    }

    private companion object {
        const val TAG = "CopyRead"

        /** How long it waits for focus. */
        const val DEADLINE_MS = 2_000L
    }
}
