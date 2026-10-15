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
 * reachable from the computer.
 */
object PhoneFolders {
    private const val PREFS = "pocketdesk-phone-folders"
    private const val KEY = "folders"
    private const val NEXT_ID = "next_id"

    /** A shared folder: its id on the wire, its tree URI and its name. */
    data class Folder(val id: Int, val uri: String, val name: String)

    fun all(ctx: Context): List<Folder> =
        runCatching {
            val array = JSONArray(ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "[]"))
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Folder(o.getInt("id"), o.getString("uri"), o.getString("name"))
            }
        }.getOrDefault(emptyList())

    private fun save(ctx: Context, folders: List<Folder>) {
        val array = JSONArray()
        folders.forEach { array.put(JSONObject().put("id", it.id).put("uri", it.uri).put("name", it.name)) }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putString(KEY, array.toString()) }
    }

    /** Share the folder the picker returned, keeping its read grant. */
    fun add(ctx: Context, tree: Uri, name: String) {
        runCatching { ctx.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        val folders = all(ctx)
        if (folders.any { it.uri == tree.toString() }) return
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val id = PhoneFilesRule.nextId(folders, prefs.getInt(NEXT_ID, 0))
        prefs.edit { putInt(NEXT_ID, id + 1) }
        save(ctx, folders + Folder(id, tree.toString(), name))
    }

    /** Stop sharing a folder, and give its grant back. */
    fun remove(ctx: Context, folder: Folder) {
        save(ctx, all(ctx).filterNot { it.id == folder.id })
        runCatching {
            ctx.contentResolver.releasePersistableUriPermission(folder.uri.toUri(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }
}
