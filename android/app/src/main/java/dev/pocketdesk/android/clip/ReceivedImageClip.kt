package dev.pocketdesk.android.clip

import android.content.Context
import android.net.Uri
import android.util.Log
import dev.pocketdesk.android.ui.ClipboardImages
import java.io.InputStream

/**
 * O7: an image the computer sent, saved at its place, also put on this
 * phone's clipboard as a copy of its own (N4's provider), so the clipboard's
 * watchers know it as ours and never send it back.
 */
object ReceivedImageClip {
    private const val TAG = "PocketdeskClip"

    /** Put the image saved at [uri] on the clipboard, if it is a PNG or a JPEG. Off the main thread. */
    fun put(ctx: Context, uri: Uri) {
        runCatching {
            val head = ctx.contentResolver.openInputStream(uri)?.use { readHead(it, ReceivedImageRule.HEAD) } ?: return
            val kind = ReceivedImageRule.kind(head) ?: return
            val file = ctx.contentResolver.openInputStream(uri)?.use {
                ClipboardImages.keepCopy(ctx, it, kind.extension, ReceivedImageRule.MAX_BYTES)
            } ?: return
            ClipboardImages.writeLocal(ctx, file)
            Log.i(TAG, "a received ${kind.name} is on the clipboard")
        }.onFailure { Log.w(TAG, "a received image could not go onto the clipboard: ${it.message}") }
    }

    private fun readHead(input: InputStream, n: Int): ByteArray {
        val head = ByteArray(n)
        var read = 0
        while (read < n) {
            val got = input.read(head, read, n - read)
            if (got < 0) break
            read += got
        }
        return head.copyOf(read)
    }
}
