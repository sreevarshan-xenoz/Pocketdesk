package dev.pocketdesk.android

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.links.LinkRule
import dev.pocketdesk.android.service.TransferService
import dev.pocketdesk.android.shortcuts.ShortcutPlan
import dev.pocketdesk.android.shortcuts.Shortcuts

/**
 * Share target (Tier 1 #4, F7): text goes to the desktop clipboard; one or
 * more files go to the desktop's Downloads folder, sent by [TransferService]
 * so the transfer outlives this screen, which closes at once.
 *
 * Requires a saved host ([HostStore]) — the shared item never picks a
 * destination itself. Another app (an automation app, T7b) may name one of
 * the saved computers with the extra [Shortcuts.EXTRA_ADDRESS]; without it,
 * the last computer used.
 *
 * Shared through its "Open on computer" entry (T8a), text is not copied: its
 * first web link opens in the computer's browser.
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RustCore.start(this)
        handle(intent)
    }

    // A share made while this screen is still closing arrives here, not as
    // a new screen; it must not be dropped.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent) {
        // Present at all counts as naming one: an extra of the wrong type is
        // refused, not taken for "the last computer".
        val asked = if (intent.hasExtra(Shortcuts.EXTRA_ADDRESS)) {
            intent.getStringExtra(Shortcuts.EXTRA_ADDRESS).orEmpty()
        } else {
            null
        }
        val host = when (val share = ShortcutPlan.shareHost(asked, HostStore.hosts(this), Prefs.othersChooseComputer(this))) {
            is ShortcutPlan.Share.To -> share.host
            ShortcutPlan.Share.NoneSaved -> return done(getString(R.string.share_no_host))
            ShortcutPlan.Share.NotSaved -> return done(getString(R.string.share_not_saved))
            ShortcutPlan.Share.NotAllowed -> return done(getString(R.string.share_choosing_off))
        }

        val uris = sharedUris(intent)
        val text =
            if (intent.type == "text/plain") intent.getStringExtra(Intent.EXTRA_TEXT) else null
        if (intent.component?.className == OPEN_ON_COMPUTER) {
            val url = text?.let { LinkRule.find(it) }
                ?: return done(getString(R.string.link_not_web))
            return sendLink(host, url)
        }
        when {
            uris.isNotEmpty() && !ShortcutPlan.shareable(uris.map { it.scheme }) ->
                done(getString(R.string.share_not_content))
            uris.isNotEmpty() -> {
                // The service reads the files under this screen's grant. A
                // share that gave no permission to read them (the sender
                // did not grant it) is refused by the system here, and must
                // end with a message, not a crash.
                try {
                    TransferService.start(this, host.address, host.controlPort, uris)
                } catch (e: SecurityException) {
                    android.util.Log.w("ShareActivity", "Shared files are not readable", e)
                    done(getString(R.string.share_unreadable))
                    return
                }
                done(resources.getQuantityString(R.plurals.share_sending_files, uris.size, uris.size, host.address))
            }
            !text.isNullOrBlank() -> sendText(host, text)
            else -> done(getString(R.string.share_nothing))
        }
    }

    private fun sharedUris(intent: Intent): List<Uri> = when (intent.action) {
        Intent.ACTION_SEND_MULTIPLE ->
            @Suppress("DEPRECATION")
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
        else ->
            listOfNotNull(@Suppress("DEPRECATION") intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
    }

    private fun sendText(host: HostStore.Host, text: String) {
        Thread {
            val result = RustCore.sendClipboard(host.address, host.controlPort, text)
            runOnUiThread {
                done(
                    if (result.isSuccess) {
                        getString(R.string.share_sent_clipboard)
                    } else {
                        getString(R.string.share_failed, result.exceptionOrNull()?.message)
                    },
                )
            }
        }.start()
    }

    private fun sendLink(host: HostStore.Host, url: String) {
        Thread {
            val result = RustCore.sendLink(host.address, host.controlPort, url)
            runOnUiThread {
                done(
                    if (result.isSuccess) {
                        getString(R.string.link_sent)
                    } else {
                        getString(R.string.share_failed, result.exceptionOrNull()?.message)
                    },
                )
            }
        }.start()
    }

    /** Say [message] where it outlives this screen, and close it. */
    private fun done(message: String) {
        Toast.makeText(applicationContext, message, Toast.LENGTH_LONG).show()
        finish()
    }

    private companion object {
        /** The manifest's "Open on computer" alias of this screen. */
        const val OPEN_ON_COMPUTER = "dev.pocketdesk.android.OpenOnComputer"
    }
}
