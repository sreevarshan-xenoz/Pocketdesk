package dev.pocketdesk.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Log
import androidx.core.content.FileProvider
import java.io.ByteArrayOutputStream
import java.io.File
import java.security.MessageDigest

/**
 * When an image moves between the phone's clipboard and the desktop's (N4).
 *
 * Both ends know an image by the hex SHA-256 of its PNG, so the one that
 * just moved is never sent back: the desktop's reply naming the image this
 * phone sent is not fetched, and an image this app put on the clipboard
 * itself (its own FileProvider URI) is not sent.
 */
class ImageClipRule {
    private var lastId: String? = null

    fun shouldSend(id: String, ours: Boolean): Boolean = !ours && id != lastId

    fun shouldFetch(id: String, size: Long): Boolean = id != lastId && size in 1..MAX_BYTES

    /** [id] crossed, either way. */
    fun moved(id: String) {
        lastId = id
    }

    companion object {
        /** The largest image either side moves, as PNG bytes. */
        const val MAX_BYTES: Long = 32L * 1024 * 1024

        private val PNG_SIGNATURE =
            byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 0x0D, 0x0A, 0x1A, 0x0A)

        fun idOf(png: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(png).joinToString("") { "%02x".format(it) }

        fun isPng(bytes: ByteArray): Boolean =
            bytes.size >= PNG_SIGNATURE.size && PNG_SIGNATURE.indices.all { bytes[it] == PNG_SIGNATURE[it] }
    }
}

/** The phone's side of clipboard images: reading, staging and writing them. */
object ClipboardImages {
    private const val TAG = "ClipboardImages"

    fun authority(ctx: Context): String = "${ctx.packageName}.clipboard"

    /** The image on the phone's clipboard, if it holds one. */
    fun localImageUri(ctx: Context): Uri? {
        val clip = ctx.getSystemService(ClipboardManager::class.java)?.primaryClip ?: return null
        if (clip.itemCount == 0 || !clip.description.hasMimeType("image/*")) return null
        return clip.getItemAt(0).uri
    }

    fun isOurs(ctx: Context, uri: Uri): Boolean = uri.authority == authority(ctx)

    /**
     * The image at [uri] as PNG: its own bytes when it already is one,
     * re-encoded otherwise. `null` when it cannot be read or is too large.
     */
    fun pngOf(ctx: Context, uri: Uri): ByteArray? = runCatching {
        val raw = ctx.contentResolver.openInputStream(uri)?.use { input ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                out.write(buffer, 0, n)
                // Larger than any image this would send, even re-encoded.
                if (out.size() > 4 * ImageClipRule.MAX_BYTES) return@runCatching null
            }
            out.toByteArray()
        } ?: return@runCatching null
        val png = if (ImageClipRule.isPng(raw)) {
            raw
        } else {
            val bitmap = BitmapFactory.decodeByteArray(raw, 0, raw.size) ?: return@runCatching null
            ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
        }
        png.takeIf { it.size in 1..ImageClipRule.MAX_BYTES }
    }.onFailure { Log.w(TAG, "could not read the clipboard image", it) }.getOrNull()

    /** A file to send [png] from (the bridge reads a path). */
    fun stage(ctx: Context, png: ByteArray): File =
        File(ctx.cacheDir, "clipboard-out").apply { mkdirs() }.resolve("out.png").apply { writeBytes(png) }

    /** Where the desktop's image [id] is received; older ones are removed. */
    fun receivedFile(ctx: Context, id: String): File {
        val dir = File(ctx.cacheDir, "clipboard").apply { mkdirs() }
        dir.listFiles()?.forEach { if (it.name != "$id.png") it.delete() }
        return dir.resolve("$id.png")
    }

    /** Put the received image [file] on the phone's clipboard. */
    fun writeLocal(ctx: Context, file: File) {
        val cm = ctx.getSystemService(ClipboardManager::class.java) ?: return
        val uri = FileProvider.getUriForFile(ctx, authority(ctx), file)
        cm.setPrimaryClip(ClipData.newUri(ctx.contentResolver, "Pocketdesk", uri))
    }
}
