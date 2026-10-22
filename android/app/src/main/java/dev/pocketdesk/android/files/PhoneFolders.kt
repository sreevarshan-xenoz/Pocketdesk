package dev.pocketdesk.android.files

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.net.toUri
import androidx.core.content.edit
import org.json.JSONArray
import org.json.JSONObject

/**
 * The folders the computer may open (T10g): each picked with Android's
 * folder picker, a persisted read-only grant. Nothing else on the phone is
 * reachable from the computer. A folder its owner marks (N6) also keeps a
 * write grant, for the computer to add files and folders there.
 */
object PhoneFolders {
    private const val PREFS = "pocketdesk-phone-folders"
    private const val KEY = "folders"
    private const val NEXT_ID = "next_id"

    /**
     * A shared folder: its id on the wire, its tree URI, its name, and
     * whether the computer may add to it (N6).
     */
    data class Folder(val id: Int, val uri: String, val name: String, val add: Boolean = false)

    /**
     * The shared folders the phone can still read: one whose grant is gone
     * (taken back in Android's settings, or its storage removed) is dropped.
     */
    fun all(ctx: Context): List<Folder> {
        val saved = stored(ctx)
        // None shared: no call to the system for the grants (N7 review).
        if (saved.isEmpty()) return saved
        val grants = runCatching { ctx.contentResolver.persistedUriPermissions }.getOrNull() ?: return saved
        val granted = grants.filter { it.isReadPermission }.map { it.uri.toString() }.toSet()
        val writable = grants.filter { it.isWritePermission }.map { it.uri.toString() }.toSet()
        // A mark whose write grant is gone is a mark no more (N6).
        val readable = saved.filter { it.uri in granted }.map { if (it.add && it.uri !in writable) it.copy(add = false) else it }
        if (readable != saved) save(ctx, readable)
        return readable
    }

    private fun stored(ctx: Context): List<Folder> =
        runCatching {
            val array = JSONArray(ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "[]"))
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Folder(o.getInt("id"), o.getString("uri"), o.getString("name"), o.optBoolean("add", false))
            }
        }.getOrDefault(emptyList())

    private fun save(ctx: Context, folders: List<Folder>) {
        val array = JSONArray()
        folders.forEach {
            array.put(JSONObject().put("id", it.id).put("uri", it.uri).put("name", it.name).put("add", it.add))
        }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putString(KEY, array.toString()) }
    }

    /** Share the folder the picker returned, keeping its read grant; false when Android keeps none. */
    fun add(ctx: Context, tree: Uri, name: String): Boolean {
        runCatching { ctx.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
            .onFailure { return false }
        val folders = all(ctx)
        if (folders.any { it.uri == tree.toString() }) return true
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val id = PhoneFilesRule.nextId(folders, prefs.getInt(NEXT_ID, 0))
        prefs.edit { putInt(NEXT_ID, id + 1) }
        save(ctx, folders + Folder(id, tree.toString(), name))
        return true
    }

    /**
     * Whether [uri] is also where files from computers go (C2): its grants
     * are that setting's too, and kept (grants are per folder, not per
     * feature; N6 review).
     */
    private fun receivesFiles(ctx: Context, uri: String): Boolean =
        dev.pocketdesk.android.Prefs.downloadTree(ctx) == uri

    /** Stop sharing a folder, and give its grants back. */
    fun remove(ctx: Context, folder: Folder) {
        save(ctx, all(ctx).filterNot { it.id == folder.id })
        if (receivesFiles(ctx, folder.uri)) return
        val flags = if (folder.add) {
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        } else {
            Intent.FLAG_GRANT_READ_URI_PERMISSION
        }
        runCatching { ctx.contentResolver.releasePersistableUriPermission(folder.uri.toUri(), flags) }
    }

    /**
     * Let the computer add to [folder] (N6), with the grant the picker just
     * gave for [picked]: false when it is another folder, or Android keeps
     * no write grant for it.
     */
    fun markAdd(ctx: Context, folder: Folder, picked: Uri): Boolean {
        if (picked.toString() != folder.uri) return false
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        runCatching { ctx.contentResolver.takePersistableUriPermission(picked, flags) }.onFailure { return false }
        save(ctx, all(ctx).map { if (it.id == folder.id) it.copy(add = true) else it })
        return true
    }

    /** The computer adds nothing more to [folder] (N6): its write grant goes back. */
    fun unmarkAdd(ctx: Context, folder: Folder) {
        save(ctx, all(ctx).map { if (it.id == folder.id) it.copy(add = false) else it })
        if (receivesFiles(ctx, folder.uri)) return
        runCatching {
            ctx.contentResolver.releasePersistableUriPermission(folder.uri.toUri(), Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        }
    }
}
