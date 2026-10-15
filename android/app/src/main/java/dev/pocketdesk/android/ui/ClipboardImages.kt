package dev.pocketdesk.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import androidx.exifinterface.media.ExifInterface
import android.net.Uri
import android.util.Log
import androidx.core.content.FileProvider
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import java.security.MessageDigest

/**
 * When an image moves between the phone's clipboard and the desktop's (N4).
 *
 * Both ends know an image by the hex SHA-256 of its PNG, so the one that
 * just moved is never sent back: the desktop's reply naming the image this
 * phone sent is not fetched, and an image this app put on the clipboard
 * itself (its own FileProvider URI) is not sent. Text moving in between
 * clears that memory, so the same image copied again moves again. A fetch
 * that failed waits [RETRY_MS] before that image is tried again.
 */
class ImageClipRule {
    private var lastId: String? = null
    private var failedId: String? = null
    private var failedAtMs = 0L

    fun shouldSend(id: String, ours: Boolean): Boolean = !ours && id != lastId

    fun shouldFetch(id: String, size: Long, nowMs: Long): Boolean =
        id != lastId &&
            size in 1..MAX_BYTES &&
            !(id == failedId && nowMs - failedAtMs < RETRY_MS)

    /** [id] crossed, either way. */
    fun moved(id: String) {
        lastId = id
        failedId = null
    }

    /** Text crossed, either way: the last image is no longer on either clipboard. */
    fun textMoved() {
        lastId = null
    }

    fun fetchFailed(id: String, nowMs: Long) {
        failedId = id
        failedAtMs = nowMs
    }

    companion object {
        /** The largest image either side moves, as PNG bytes. */
        const val MAX_BYTES: Long = 32L * 1024 * 1024

        /** How long a failed image waits before it is tried again. */
        const val RETRY_MS: Long = 60_000

        /** A photo is decoded at most this many pixels on its long side. */
        const val MAX_SIDE = 4096

        private val PNG_SIGNATURE =
            byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 0x0D, 0x0A, 0x1A, 0x0A)

        fun idOf(png: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(png).joinToString("") { "%02x".format(it) }

        fun isPng(bytes: ByteArray): Boolean =
            bytes.size >= PNG_SIGNATURE.size && PNG_SIGNATURE.indices.all { bytes[it] == PNG_SIGNATURE[it] }

        /** The power-of-two `inSampleSize` that brings the long side within [MAX_SIDE]. */
        fun sampleFor(width: Int, height: Int): Int {
            var sample = 1
            while (maxOf(width, height) / sample > MAX_SIDE) sample *= 2
            return sample
        }
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

    private fun readUpTo(input: InputStream, limit: Long): ByteArray? {
        val out = ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        while (true) {
            val n = input.read(buffer)
            if (n < 0) return out.toByteArray()
            out.write(buffer, 0, n)
            if (out.size() > limit) return null
        }
    }

    /**
     * The image at [uri] as PNG: its own bytes when it already is a PNG that
     * fits, else decoded at most [ImageClipRule.MAX_SIDE] pixels on its long
     * side, turned upright from its EXIF orientation, and encoded (halved
     * again until it fits). `null` when it cannot be read.
     */
    fun pngOf(ctx: Context, uri: Uri): ByteArray? = runCatching {
        val resolver = ctx.contentResolver
        val head = resolver.openInputStream(uri)?.use { input ->
            ByteArray(8).also { if (input.read(it) < it.size) return@runCatching null }
        } ?: return@runCatching null
        if (ImageClipRule.isPng(head)) {
            resolver.openInputStream(uri)?.use { readUpTo(it, ImageClipRule.MAX_BYTES) }
                ?.let { return@runCatching it }
        }
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return@runCatching null
        val rotation = resolver.openInputStream(uri)?.use { input ->
            when (ExifInterface(input).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
                ExifInterface.ORIENTATION_ROTATE_90 -> 90f
                ExifInterface.ORIENTATION_ROTATE_180 -> 180f
                ExifInterface.ORIENTATION_ROTATE_270 -> 270f
                else -> 0f
            }
        } ?: 0f
        var sample = ImageClipRule.sampleFor(bounds.outWidth, bounds.outHeight)
        repeat(3) {
            val options = BitmapFactory.Options().apply { inSampleSize = sample }
            val decoded = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }
                ?: return@runCatching null
            val upright = if (rotation == 0f) {
                decoded
            } else {
                Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, Matrix().apply { postRotate(rotation) }, true)
                    .also { if (it !== decoded) decoded.recycle() }
            }
            val png = ByteArrayOutputStream().also { upright.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            upright.recycle()
            if (png.size <= ImageClipRule.MAX_BYTES) return@runCatching png
            sample *= 2
        }
        null
    }.onFailure { Log.w(TAG, "could not read the clipboard image", it) }.getOrNull()

    /**
     * Send [png] from a file (the bridge reads a path), and remove the file
     * after. Each send its own file: a session's sync and a screenshot
     * (T10h) may stage at once.
     */
    fun <T> withStaged(ctx: Context, png: ByteArray, send: (File) -> T): T {
        val file = File.createTempFile("out", ".png", File(ctx.cacheDir, "clipboard-out").apply { mkdirs() })
        file.writeBytes(png)
        try {
            return send(file)
        } finally {
            file.delete()
        }
    }

    private fun receivedDir(ctx: Context): File = File(ctx.cacheDir, "clipboard").apply { mkdirs() }

    /** Where the desktop's image [id] is downloaded before it is kept. */
    fun partFile(ctx: Context, id: String): File = receivedDir(ctx).resolve("$id.part")

    /**
     * Keep the downloaded [part] as the image [id], and remove the ones
     * before it: only now, so a failed download never takes away the file
     * the clipboard still points at.
     */
    fun keepReceived(ctx: Context, part: File, id: String): File {
        val file = receivedDir(ctx).resolve("$id.png")
        if (!part.renameTo(file)) error("could not keep the received image")
        receivedDir(ctx).listFiles()?.forEach { if (it.name != file.name) it.delete() }
        return file
    }

    /** Put the received image [file] on the phone's clipboard. */
    fun writeLocal(ctx: Context, file: File) {
        val cm = ctx.getSystemService(ClipboardManager::class.java) ?: return
        val uri = FileProvider.getUriForFile(ctx, authority(ctx), file)
        cm.setPrimaryClip(ClipData.newUri(ctx.contentResolver, "Pocketdesk", uri))
    }
}
