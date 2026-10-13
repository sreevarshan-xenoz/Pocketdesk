package dev.pocketdesk.android

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.service.TransferService

/**
 * Share target (Tier 1 #4, F7): text goes to the desktop clipboard; one or
 * more files go to the desktop's Downloads folder, sent by [TransferService]
 * so the transfer outlives this screen, which closes at once.
 *
 * Requires a saved host ([HostStore]) — the shared item never picks a
 * destination itself.
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RustCore.start(filesDir)
        handle(intent)
    }

    // A share made while this screen is still closing arrives here, not as
    // a new screen; it must not be dropped.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent) {
        val host = HostStore.lastHost(this)
        if (host == null) {
            done(getString(R.string.share_no_host))
            return
        }

        val uris = sharedUris(intent)
        val text =
            if (intent.type == "text/plain") intent.getStringExtra(Intent.EXTRA_TEXT) else null
        when {
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

    /** Say [message] where it outlives this screen, and close it. */
    private fun done(message: String) {
        Toast.makeText(applicationContext, message, Toast.LENGTH_LONG).show()
        finish()
    }
}
