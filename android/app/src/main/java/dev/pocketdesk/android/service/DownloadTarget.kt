package dev.pocketdesk.android.service

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.os.StatFs
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import androidx.core.net.toUri
import java.io.File

/**
 * Where a file from the computer is written (F7b): Downloads/Pocketdesk,
 * or a folder the owner picked (C2), through the Storage Access Framework,
 * as `<name>.part` until it is whole.
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
        /** A document in a picked folder (C2), named as a part until published. */
        private var documentUri: Uri? = null,
    ) {
        /** What a viewer can open once published, if anything. */
        val viewUri: Uri? get() = mediaUri ?: documentUri

        /** Where the bytes are, to find this file again for a retry (T6a). */
        val location: String get() = mediaUri?.toString() ?: documentUri?.toString() ?: legacyFile?.absolutePath ?: ""

        /** Let go of the descriptor and keep the file, unpublished, for a retry. */
        fun keepForRetry() {
            runCatching { descriptor.close() }
        }

        fun publish() {
            documentUri?.let { document ->
                // The part takes its name; a provider that cannot rename
                // leaves the file whole under the part's name.
                val part = displayName(context, document) ?: return
                if (!part.endsWith(DownloadPlace.PART_SUFFIX)) return
                // A name taken since the part was made: the next free one.
                val renamed = DownloadPlace.candidates(DownloadPlace.finishedName(part)).take(20).firstNotNullOfOrNull { name ->
                    runCatching { DocumentsContract.renameDocument(context.contentResolver, document, name) }.getOrNull()
                }
                renamed?.let { documentUri = it }
                return
            }
            val uri = mediaUri ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val done = ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }
                context.contentResolver.update(uri, done, null, null)
            }
        }

        /**
         * Date the published file [ms] (K1); whether it took. A file in
         * Downloads/Pocketdesk is the app's own, so its time can be set
         * through its path; a picked folder's document has no way to.
         */
        fun date(ms: Long): Boolean = runCatching {
            legacyFile?.let { return@runCatching it.setLastModified(ms) }
            val uri = mediaUri ?: return@runCatching false
            @Suppress("DEPRECATION") // read only, for the app's own file
            val path = context.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns.DATA), null, null, null)
                ?.use { if (it.moveToFirst()) it.getString(0) else null }
            if (path != null && File(path).setLastModified(ms)) {
                // MediaStore's own date follows the file's at its next look.
                android.media.MediaScannerConnection.scanFile(context, arrayOf(path), null, null)
                return@runCatching true
            }
            // No path the app may touch (Android 10 without legacy storage):
            // MediaStore's own date at least, which Files apps sort by
            // (K1 review).
            val row = ContentValues().apply { put(MediaStore.MediaColumns.DATE_MODIFIED, ms / 1000) }
            context.contentResolver.update(uri, row, null, null) > 0
        }.getOrDefault(false)

        fun discard() {
            runCatching { descriptor.close() }
            mediaUri?.let { runCatching { context.contentResolver.delete(it, null, null) } }
            documentUri?.let { runCatching { DocumentsContract.deleteDocument(context.contentResolver, it) } }
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
        // The picked folder first (C2); one that can no longer be written
        // (its grant taken back, or the folder gone) means Downloads/Pocketdesk.
        fellBack = false
        tree(context)?.let { tree ->
            runCatching { createInTree(context, tree, safe, folders) }.getOrNull()?.let { return@runCatching it }
            fellBack = true
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            createMedia(context, safe, folders)
        } else {
            createLegacy(context, safe, folders)
        }
    }.getOrNull()

    /**
     * A new, empty file named like [name] in the shared folder [tree] (N6),
     * under [folders] (found or made), as a part renamed once whole; `null`
     * when none can be made.
     */
    fun createIn(context: Context, tree: Uri, name: String, folders: List<String>): Target? =
        runCatching { createInTree(context, tree, DownloadName.safe(name), folders) }.getOrNull()

    /**
     * A name for a folder from the computer under Downloads/Pocketdesk that
     * none of this app's downloads use yet: [base], else `base (1)`…
     */
    fun freeFolder(context: Context, base: String): String =
        FolderWalk.freeName(base) { candidate ->
            val tree = tree(context)
            if (tree != null) {
                child(context, tree, DocumentsContract.getTreeDocumentId(tree), candidate) != null
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
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
            val document = DownloadPlace.isDocument(location)
            context.contentResolver.openFileDescriptor(uri, "rw")?.let {
                if (document) Target(it, context, null, null, uri) else Target(it, context, uri, null)
            }
        } else {
            val file = File(location).takeIf { it.isFile } ?: return@runCatching null
            Target(ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_WRITE), context, null, file)
        }
    }.getOrNull()

    /** Delete an unfinished file at [location] that is no longer wanted. */
    fun delete(context: Context, location: String) {
        runCatching {
            if (location.startsWith("content://")) {
                val uri = location.toUri()
                if (DownloadPlace.isDocument(location)) {
                    DocumentsContract.deleteDocument(context.contentResolver, uri)
                } else {
                    context.contentResolver.delete(uri, null, null)
                }
            } else {
                File(location).delete()
            }
        }
    }

    /**
     * Whether there is room for [size] bytes, with a margin for the system.
     * A picked folder may be on another volume (an SD card, a drive, a
     * cloud): there the write itself says (C2 review).
     */
    fun hasRoomFor(context: Context, size: Long): Boolean {
        if (tree(context) != null) return true
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

    /** The writes this app still holds on picked folders. */
    private fun granted(context: Context): List<String> =
        context.contentResolver.persistedUriPermissions.filter { it.isWritePermission }.map { it.uri.toString() }

    /** The picked folder files go to, while it can still be written (C2). */
    fun tree(context: Context): Uri? =
        DownloadPlace.choose(dev.pocketdesk.android.Prefs.downloadTree(context), granted(context))?.toUri()

    /** A folder was picked but can no longer be written: files went to Downloads/Pocketdesk. */
    fun treeLost(context: Context): Boolean =
        DownloadPlace.lost(dev.pocketdesk.android.Prefs.downloadTree(context), granted(context))

    /** The last [create] meant the picked folder, which took nothing (it is gone): the file went to Downloads/Pocketdesk. */
    @Volatile
    var fellBack = false
        private set

    /** What the picked folder is called, for Settings. */
    fun treeName(context: Context, tree: Uri): String? = runCatching {
        displayName(context, DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree)))
    }.getOrNull()

    private fun displayName(context: Context, document: Uri): String? = runCatching {
        context.contentResolver.query(document, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull()

    private fun canRename(context: Context, document: Uri): Boolean = runCatching {
        context.contentResolver.query(document, arrayOf(DocumentsContract.Document.COLUMN_FLAGS), null, null, null)
            ?.use { it.moveToFirst() && it.getInt(0) and DocumentsContract.Document.FLAG_SUPPORTS_RENAME != 0 }
            ?: false
    }.getOrDefault(false)

    /** The child of [parent] in [tree] named [name], by its document id. */
    private fun child(context: Context, tree: Uri, parent: String, name: String): String? {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent)
        return context.contentResolver.query(
            children,
            arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            null,
            null,
            null,
        )?.use { cursor ->
            var found: String? = null
            while (found == null && cursor.moveToNext()) {
                if (cursor.getString(1) == name) found = cursor.getString(0)
            }
            found
        }
    }

    /** A new part in [tree], under [folders] (found or made). */
    private fun createInTree(context: Context, tree: Uri, name: String, folders: List<String>): Target? {
        val resolver = context.contentResolver
        var parent = DocumentsContract.getTreeDocumentId(tree)
        for (folder in folders) {
            parent = child(context, tree, parent, folder)
                ?: DocumentsContract.createDocument(
                    resolver,
                    DocumentsContract.buildDocumentUriUsingTree(tree, parent),
                    DocumentsContract.Document.MIME_TYPE_DIR,
                    folder,
                )?.let { DocumentsContract.getDocumentId(it) }
                ?: return null
        }
        // A name the folder does not hold yet, so the part's rename finds it
        // free (a provider refuses a rename onto a name it holds).
        val folder = parent
        val free = DownloadPlace.freeName(name) { child(context, tree, folder, it) != null }
        // A plain type for the part: a provider adds the extension a type
        // implies, and the finished name brings its own.
        val dir = DocumentsContract.buildDocumentUriUsingTree(tree, parent)
        var document = DocumentsContract.createDocument(
            resolver,
            dir,
            "application/octet-stream",
            DownloadPlace.partName(free),
        ) ?: return null
        // A provider that cannot rename keeps a part's name for good: the
        // file is written under its own name there instead.
        if (!canRename(context, document)) {
            runCatching { DocumentsContract.deleteDocument(resolver, document) }
            document = DocumentsContract.createDocument(resolver, dir, mimeOf(free), free) ?: return null
        }
        val descriptor = runCatching { resolver.openFileDescriptor(document, "rw") }.getOrNull()
        if (descriptor == null) {
            runCatching { DocumentsContract.deleteDocument(resolver, document) }
            return null
        }
        return Target(descriptor, context, null, null, document)
    }

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
