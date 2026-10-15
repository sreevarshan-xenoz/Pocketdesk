package dev.pocketdesk.android.files

import android.content.Context
import android.net.Uri
import androidx.core.net.toUri
import android.os.SystemClock
import android.provider.DocumentsContract
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.service.TransferService
import org.json.JSONArray
import org.json.JSONObject

/**
 * The computer browsing this phone's shared folders (T10g), run on the
 * notification listener's tick: it keeps the control link while folders are
 * shared, answers a list with the folder's entries, and answers a fetch by
 * sending that file to the computer the way the share sheet does.
 */
class PhoneFiles(private val ctx: Context) {
    private var lastConnectMs = 0L

    /** A request as the bridge kept it. */
    private data class Request(val host: String, val port: Int, val fetch: Boolean, val root: Int, val path: List<String>)

    fun tick() {
        val folders = PhoneFolders.all(ctx)
        val own = PhoneLink.target(ctx)
        if (folders.isNotEmpty() && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        for (request in takeRequests()) {
            if (!PhoneFilesRule.fromOwnComputer(request.host, request.port, own?.address, own?.controlPort)) continue
            if (!PhoneFilesRule.validPath(request.path)) continue
            runCatching { if (request.fetch) fetch(request, folders) else list(request, folders) }
                .onFailure { Log.w(TAG, "a request for this phone's files failed", it) }
        }
    }

    private fun takeRequests(): List<Request> =
        runCatching {
            val array = JSONArray(RustCore.takePhoneFileRequests())
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                val path = o.getJSONArray("path")
                Request(
                    o.getString("host"),
                    o.getInt("port"),
                    o.getBoolean("fetch"),
                    o.getInt("root"),
                    (0 until path.length()).map { path.getString(it) },
                )
            }
        }.getOrDefault(emptyList())

    /** The document `path` names under the shared folder, child by child, looking through every child. */
    private fun resolve(tree: Uri, path: List<String>): Pair<String, Boolean>? {
        var docId = DocumentsContract.getTreeDocumentId(tree)
        var dir = true
        for (name in path) {
            var child: Child? = null
            eachChild(tree, docId) {
                if (it.name == name) child = it
                child == null
            }
            val found = child ?: return null
            docId = found.docId
            dir = found.dir
        }
        return docId to dir
    }

    private data class Child(val docId: String, val name: String, val dir: Boolean, val size: Long, val modified: Long)

    /** Each child of [docId], to [take] until it answers false, at most [MAX_SCANNED]; false if any were left. */
    private fun eachChild(tree: Uri, docId: String, take: (Child) -> Boolean): Boolean {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        val columns =
            arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE,
                DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            )
        // No cursor is a folder the phone can no longer read (its grant gone), not an empty one.
        val cursor = ctx.contentResolver.query(uri, columns, null, null, null) ?: error("no cursor for $uri")
        cursor.use { c ->
            var scanned = 0
            while (c.moveToNext()) {
                if (++scanned > MAX_SCANNED) return false
                val name = c.getString(1) ?: continue
                val child =
                    Child(
                        c.getString(0),
                        name,
                        c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR,
                        if (c.isNull(3)) 0 else c.getLong(3),
                        if (c.isNull(4)) 0 else c.getLong(4),
                    )
                if (!take(child)) return true
            }
        }
        return true
    }

    /**
     * Answer with the listing of the folder [request] names; [note] is said
     * with it (a fetch that failed). A folder the phone cannot read is
     * answered with why, never left unanswered.
     */
    private fun list(request: Request, folders: List<PhoneFolders.Folder>, note: String? = null) {
        val roots = JSONArray()
        folders.forEach {
            roots.put(JSONObject().put("id", it.id).put("name", PhoneFilesRule.rootName(it.name, "Folder ${it.id + 1}")))
        }
        val body = JSONObject().put("roots", roots).put("path", JSONArray(request.path))
        val folder = PhoneFilesRule.root(folders, request.root) ?: folders.firstOrNull()
        if (folder != null) {
            body.put("root", folder.id)
            val tree = folder.uri.toUri()
            val asked = folder.id == request.root
            if (!asked) body.put("path", JSONArray())
            val found = runCatching { resolve(tree, if (asked) request.path else emptyList()) }
            val target = found.getOrNull()
            when {
                found.isFailure -> body.put("error", UNREADABLE)
                target == null -> body.put("error", "Not on the phone any more")
                !target.second -> body.put("error", "Not a folder")
                else -> runCatching {
                    val listed = mutableListOf<Child>()
                    val whole = eachChild(tree, target.first) {
                        if (PhoneFilesRule.listed(it.name)) listed += it
                        true
                    }
                    val entries = JSONArray()
                    listed
                        .sortedWith(compareBy({ !it.dir }, { it.name.lowercase() }))
                        .take(PhoneFilesRule.MAX_ENTRIES)
                        .forEach {
                            entries.put(
                                JSONObject().put("name", it.name).put("dir", it.dir).put("size", it.size).put("modified", it.modified),
                            )
                        }
                    body.put("entries", entries)
                    body.put("truncated", !whole || listed.size > PhoneFilesRule.MAX_ENTRIES)
                }.onFailure {
                    Log.w(TAG, "could not list a shared folder", it)
                    body.put("error", UNREADABLE)
                }
            }
            if (!asked && !body.has("error")) body.put("error", "That folder is no longer shared")
        }
        if (note != null && !body.has("error")) body.put("error", note)
        RustCore.sendPhonePacket(request.host, request.port, LISTING, body.toString())
    }

    /**
     * Send the file [request] names, as the share sheet does; when it cannot
     * be sent, answer with its folder's listing and why.
     */
    private fun fetch(request: Request, folders: List<PhoneFolders.Folder>) {
        val name = request.path.lastOrNull() ?: return
        val problem =
            runCatching {
                val folder = PhoneFilesRule.root(folders, request.root) ?: return@runCatching "That folder is no longer shared"
                val tree = folder.uri.toUri()
                val (docId, dir) = resolve(tree, request.path) ?: return@runCatching "$name is not on the phone any more"
                if (dir) return@runCatching "$name is a folder"
                TransferService.start(ctx, request.host, request.port, listOf(DocumentsContract.buildDocumentUriUsingTree(tree, docId)))
                null
            }.getOrElse {
                Log.w(TAG, "could not send a shared file", it)
                "The phone could not send $name: open Pocketdesk on the phone and try again"
            }
        if (problem != null) list(request.copy(path = request.path.dropLast(1)), folders, problem)
    }

    private companion object {
        const val TAG = "PhoneFiles"
        const val LISTING = "kdeconnect.pocketdesk.phone_files.listing"
        const val RECONNECT_MS = 60_000L
        const val UNREADABLE = "The phone cannot read this folder: remove it and add it again on the phone"

        /** Children of one folder looked at, at most. */
        const val MAX_SCANNED = 100_000
    }
}
