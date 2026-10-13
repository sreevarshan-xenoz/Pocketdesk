package dev.linuxlink.android.service

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.os.StatFs
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import java.io.File

/**
 * Where a file from the computer is written (F7b): Downloads/Pocketdesk.
 *
 * On Android 10 and later it is a MediaStore download, kept pending (and so
 * invisible to other apps) until [Target.publish]. On 8 and 9 it is the
 * app's own Downloads folder, which needs no permission; that file has no
 * uri another app may open, so it gets no open action.
 */
object DownloadTarget {
    private const val FOLDER = "Pocketdesk"

    /** A file being written; [publish] or [discard] it exactly once. */
    class Target internal constructor(
        val descriptor: ParcelFileDescriptor,
        private val context: Context,
        private val mediaUri: Uri?,
        private val legacyFile: File?,
    ) {
        /** What a viewer can open once published, if anything. */
        val viewUri: Uri? get() = mediaUri

        fun publish() {
            val uri = mediaUri ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val done = ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }
                context.contentResolver.update(uri, done, null, null)
            }
        }

        fun discard() {
            runCatching { descriptor.close() }
            mediaUri?.let { runCatching { context.contentResolver.delete(it, null, null) } }
            legacyFile?.delete()
        }
    }

    /** A new, empty file named like [name], or `null` when none can be made. */
    fun create(context: Context, name: String): Target? = runCatching {
        val safe = DownloadName.safe(name)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            createMedia(context, safe)
        } else {
            createLegacy(context, safe)
        }
    }.getOrNull()

    /** Whether there is room for [size] bytes, with a margin for the system. */
    fun hasRoomFor(context: Context, size: Long): Boolean {
        val dir = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            Environment.getExternalStorageDirectory()
        } else {
            context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: return false
        }
        val free = runCatching { StatFs(dir.path).availableBytes }.getOrDefault(0L)
        return free - size > MARGIN_BYTES
    }

    private const val MARGIN_BYTES = 64L * 1024 * 1024

    private fun mimeOf(name: String): String =
        MimeTypeMap.getSingleton().getMimeTypeFromExtension(DownloadName.extension(name))
            ?: "application/octet-stream"

    private fun createMedia(context: Context, name: String): Target? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, mimeOf(name))
            put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$FOLDER")
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: return null
        val descriptor = resolver.openFileDescriptor(uri, "w")
        if (descriptor == null) {
            resolver.delete(uri, null, null)
            return null
        }
        return Target(descriptor, context, uri, null)
    }

    private fun createLegacy(context: Context, name: String): Target? {
        val base = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: return null
        val dir = File(base, FOLDER).apply { mkdirs() }
        val ext = DownloadName.extension(name).let { if (it.isEmpty()) "" else ".$it" }
        val stem = if (ext.isEmpty()) name else name.dropLast(ext.length)
        val file = generateSequence(0) { it + 1 }.take(1000)
            .map { n -> File(dir, if (n == 0) name else "$stem ($n)$ext") }
            .firstOrNull { it.createNewFile() } ?: return null
        val descriptor = ParcelFileDescriptor.open(
            file,
            ParcelFileDescriptor.MODE_WRITE_ONLY or ParcelFileDescriptor.MODE_TRUNCATE,
        )
        return Target(descriptor, context, null, file)
    }
}
