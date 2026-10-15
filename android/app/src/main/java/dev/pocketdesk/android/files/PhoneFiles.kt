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
        ctx.contentResolver.query(uri, columns, null, null, null)?.use { c ->
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

    private fun list(request: Request, folders: List<PhoneFolders.Folder>) {
        val roots = JSONArray()
        folders.forEach {
            roots.put(JSONObject().put("id", it.id).put("name", PhoneFilesRule.rootName(it.name, "Folder ${it.id + 1}")))
        }
        val body = JSONObject().put("roots", roots).put("path", JSONArray(request.path))
        val folder = PhoneFilesRule.root(folders, request.root) ?: folders.firstOrNull()
        if (folder != null) {
            body.put("root", folder.id)
            val tree = folder.uri.toUri()
            val found = if (folder.id == request.root) resolve(tree, request.path) else resolve(tree, emptyList())
            if (folder.id != request.root) body.put("path", JSONArray())
            when {
                found == null -> body.put("error", "Not on the phone any more")
                !found.second -> body.put("error", "Not a folder")
                else -> {
                    val listed = mutableListOf<Child>()
                    val whole = eachChild(tree, found.first) {
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
                }
            }
        }
        RustCore.sendPhonePacket(request.host, request.port, LISTING, body.toString())
    }

    private fun fetch(request: Request, folders: List<PhoneFolders.Folder>) {
        val folder = PhoneFilesRule.root(folders, request.root) ?: return
        val tree = folder.uri.toUri()
        val (docId, dir) = resolve(tree, request.path) ?: return
        if (dir) return
        val uri = DocumentsContract.buildDocumentUriUsingTree(tree, docId)
        TransferService.start(ctx, request.host, request.port, listOf(uri))
    }

    private companion object {
        const val TAG = "PhoneFiles"
        const val LISTING = "kdeconnect.pocketdesk.phone_files.listing"
        const val RECONNECT_MS = 60_000L

        /** Children of one folder looked at, at most. */
        const val MAX_SCANNED = 100_000
    }
}
