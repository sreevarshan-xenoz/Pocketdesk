package dev.pocketdesk.android.service

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.os.StatFs
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import androidx.core.net.toUri
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

        /** Where the bytes are, to find this file again for a retry (T6a). */
        val location: String get() = mediaUri?.toString() ?: legacyFile?.absolutePath ?: ""

        /** Let go of the descriptor and keep the file, unpublished, for a retry. */
        fun keepForRetry() {
            runCatching { descriptor.close() }
        }

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

    /**
     * A new, empty file named like [name], or `null` when none can be made.
     * [folders] are the folders under Downloads/Pocketdesk it goes in (a
     * folder from the computer, T6b), each already checked.
     */
    fun create(context: Context, name: String, folders: List<String> = emptyList()): Target? = runCatching {
        val safe = DownloadName.safe(name)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            createMedia(context, safe, folders)
        } else {
            createLegacy(context, safe, folders)
        }
    }.getOrNull()

    /**
     * A name for a folder from the computer under Downloads/Pocketdesk that
     * none of this app's downloads use yet: [base], else `base (1)`…
     */
    fun freeFolder(context: Context, base: String): String =
        FolderWalk.freeName(base) { candidate ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                folderInUse(context, candidate)
            } else {
                val root = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
                root != null && File(File(root, FOLDER), candidate).exists()
            }
        }

    /** Whether any of this app's downloads sits in Downloads/Pocketdesk/[name]. */
    private fun folderInUse(context: Context, name: String): Boolean = runCatching {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return@runCatching false
        val prefix = "${Environment.DIRECTORY_DOWNLOADS}/$FOLDER/$name/"
        val like = prefix.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_") + "%"
        context.contentResolver.query(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            arrayOf(MediaStore.Downloads._ID),
            "${MediaStore.Downloads.RELATIVE_PATH} LIKE ? ESCAPE '\\'",
            arrayOf(like),
            null,
        )?.use { it.count > 0 } ?: false
    }.getOrDefault(false)

    /**
     * The unfinished file an earlier attempt kept at [location], opened to
     * continue it, or `null` when it is gone.
     */
    fun reopen(context: Context, location: String): Target? = runCatching {
        if (location.startsWith("content://")) {
            val uri = location.toUri()
            context.contentResolver.openFileDescriptor(uri, "rw")?.let { Target(it, context, uri, null) }
        } else {
            val file = File(location).takeIf { it.isFile } ?: return@runCatching null
            Target(ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_WRITE), context, null, file)
        }
    }.getOrNull()

    /** Delete an unfinished file at [location] that is no longer wanted. */
    fun delete(context: Context, location: String) {
        runCatching {
            if (location.startsWith("content://")) {
                context.contentResolver.delete(location.toUri(), null, null)
            } else {
                File(location).delete()
            }
        }
    }

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

    private fun createMedia(context: Context, name: String, folders: List<String>): Target? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        val under = (listOf(Environment.DIRECTORY_DOWNLOADS, FOLDER) + folders).joinToString("/")
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, mimeOf(name))
            put(MediaStore.Downloads.RELATIVE_PATH, under)
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: return null
        // Read-write: a retry reads back what it has to prove it is the same file.
        val descriptor = resolver.openFileDescriptor(uri, "rw")
        if (descriptor == null) {
            resolver.delete(uri, null, null)
            return null
        }
        return Target(descriptor, context, uri, null)
    }

    private fun createLegacy(context: Context, name: String, folders: List<String>): Target? {
        val base = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: return null
        val dir = folders.fold(File(base, FOLDER)) { parent, folder -> File(parent, folder) }.apply { mkdirs() }
        val ext = DownloadName.extension(name).let { if (it.isEmpty()) "" else ".$it" }
        val stem = if (ext.isEmpty()) name else name.dropLast(ext.length)
        val file = generateSequence(0) { it + 1 }.take(1000)
            .map { n -> File(dir, if (n == 0) name else "$stem ($n)$ext") }
            .firstOrNull { it.createNewFile() } ?: return null
        val descriptor = ParcelFileDescriptor.open(
            file,
            ParcelFileDescriptor.MODE_READ_WRITE or ParcelFileDescriptor.MODE_TRUNCATE,
        )
        return Target(descriptor, context, null, file)
    }
}
