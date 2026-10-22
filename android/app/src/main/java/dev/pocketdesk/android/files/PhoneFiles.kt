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
    private data class Request(
        val host: String,
        val port: Int,
        val fetch: Boolean,
        val root: Int,
        val path: List<String>,
        /** The computer's drive asked for this file (D8): its share says so. */
        val drive: String? = null,
        /** "list", "fetch", or the drive's additions "mkdir" and "put" (N6). */
        val kind: String = if (fetch) "fetch" else "list",
        /** A put's file: its name, the computer's offer and its size (N6). */
        val name: String? = null,
        val id: String? = null,
        val size: Long = 0,
    )

    fun tick() {
        val folders = PhoneFolders.all(ctx)
        val requests = takeRequests()
        // Nothing shared and nothing asked: nothing to do (N7).
        if (folders.isEmpty() && requests.isEmpty()) return
        val own = PhoneLink.target(ctx)
        if (folders.isNotEmpty() && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        for (request in requests) {
            if (!PhoneFilesRule.fromOwnComputer(request.host, request.port, own?.address, own?.controlPort)) continue
            if (!PhoneFilesRule.reachable(request.path)) {
                // An addition named so it cannot be made is told why (N6).
                if (request.kind == "put") decline(request, AddRule.BAD_NAME)
                continue
            }
            runCatching {
                when (request.kind) {
                    "fetch" -> fetch(request, folders)
                    "mkdir" -> mkdir(request, folders)
                    "put" -> put(request, folders)
                    else -> list(request, folders)
                }
            }
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
                    o.optString("drive").takeIf { PhoneFilesRule.validDrive(it) },
                    o.optString("kind").ifEmpty { if (o.getBoolean("fetch")) "fetch" else "list" },
                    o.optString("name").ifEmpty { null },
                    o.optString("id").takeIf { PhoneFilesRule.validDrive(it) },
                    o.optLong("size", 0),
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
        val body = JSONObject().put("roots", rootsJson(folders)).put("path", JSONArray(request.path))
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

    /** The shared folders as a listing names them; a marked one says the computer may add (N6). */
    private fun rootsJson(folders: List<PhoneFolders.Folder>): JSONArray {
        val roots = JSONArray()
        folders.forEach {
            val root = JSONObject().put("id", it.id).put("name", PhoneFilesRule.rootName(it.name, "Folder ${it.id + 1}"))
            if (it.add) root.put("add", true)
            roots.put(root)
        }
        return roots
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
                TransferService.start(
                    ctx,
                    request.host,
                    request.port,
                    listOf(DocumentsContract.buildDocumentUriUsingTree(tree, docId)),
                    request.drive,
                )
                null
            }.getOrElse {
                Log.w(TAG, "could not send a shared file", it)
                "The phone could not send $name: open Pocketdesk on the phone and try again"
            }
        if (problem != null) list(request.copy(path = request.path.dropLast(1)), folders, problem)
    }

    /**
     * Make the folder [request] names in a marked folder (N6) and answer with
     * its listing; when it cannot be made, its folder's listing and why
     * ([AddRule.EXISTS] for a name taken).
     */
    private fun mkdir(request: Request, folders: List<PhoneFolders.Folder>) {
        val folder = PhoneFilesRule.root(folders, request.root)
        val parent = request.path.dropLast(1)
        val name = request.path.lastOrNull() ?: return
        val decision = AddRule.decide(true, folder?.add == true, request.path, null)
        if (folder == null || decision != AddRule.Decision.ADD) {
            refuseAt(request, folders, AddRule.reason(decision) ?: AddRule.NOT_MARKED)
            return
        }
        val tree = folder.uri.toUri()
        val problem = runCatching {
            val (docId, dir) = resolve(tree, parent) ?: return@runCatching "That folder is not on the phone any more"
            if (!dir) return@runCatching "That is not a folder"
            var taken = false
            // As a provider that ignores case would see it (N6 review).
            eachChild(tree, docId) {
                if (it.name.equals(name, ignoreCase = true)) taken = true
                !taken
            }
            if (taken) return@runCatching AddRule.EXISTS
            DocumentsContract.createDocument(
                ctx.contentResolver,
                DocumentsContract.buildDocumentUriUsingTree(tree, docId),
                DocumentsContract.Document.MIME_TYPE_DIR,
                name,
            ) ?: return@runCatching "The phone could not make $name"
            null
        }.getOrElse {
            Log.w(TAG, "could not make a folder", it)
            "The phone could not make $name"
        }
        if (problem == null) list(request, folders) else refuseAt(request, folders, problem)
    }

    /**
     * Answer [request] with why it was not done, at the path it named, so
     * the computer waiting for that path hears it (N6).
     */
    private fun refuseAt(request: Request, folders: List<PhoneFolders.Folder>, error: String) {
        val body = JSONObject()
            .put("roots", rootsJson(folders))
            .put("root", request.root)
            .put("path", JSONArray(request.path))
            .put("error", error)
        RustCore.sendPhonePacket(request.host, request.port, LISTING, body.toString())
    }

    /**
     * Fetch the computer's offer into a marked folder (N6), as a download
     * into a picked folder is written; one it may not take is declined
     * with why.
     */
    private fun put(request: Request, folders: List<PhoneFolders.Folder>) {
        val id = request.id ?: return
        val name = request.name ?: return decline(request, AddRule.BAD_NAME)
        val folder = PhoneFilesRule.root(folders, request.root)
        val decision = AddRule.decide(true, folder?.add == true, request.path, name)
        if (folder == null || decision != AddRule.Decision.ADD) {
            decline(request, AddRule.reason(decision) ?: AddRule.NOT_MARKED)
            return
        }
        TransferService.receiveInto(ctx, request.host, request.port, id, name, request.size, folder.id, request.path)
    }

    /** Decline [request]'s offer with [reason], so the computer's copy hears it. */
    private fun decline(request: Request, reason: String) {
        val id = request.id ?: return
        RustCore.declineOffer(request.host, request.port, id, reason)
            .onFailure { Log.w(TAG, "could not decline a copy: ${it.message}") }
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
