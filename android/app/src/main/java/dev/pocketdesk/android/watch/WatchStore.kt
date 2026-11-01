package dev.pocketdesk.android.watch

import android.content.Context
import androidx.core.content.edit
import dev.pocketdesk.watchlink.WatchWire
import java.util.Base64

/**
 * The watch this phone is paired with (item 9): its key, its Bluetooth
 * address (only that device may connect) and the name it gave. Private to
 * the app and left out of every backup and transfer (data_extraction_rules),
 * like the pairings with the computers: a copy on another phone must never
 * be able to reuse it. Never logged, never shown but the name.
 */
object WatchStore {
    private const val PREFS = "pocketdesk-watch"
    private const val KEY = "key"
    private const val ADDRESS = "address"
    private const val NAME = "name"

    class Paired(val key: ByteArray, val address: String, val name: String)

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun paired(ctx: Context): Paired? {
        val p = prefs(ctx)
        val key = p.getString(KEY, null)?.let { runCatching { Base64.getDecoder().decode(it) }.getOrNull() } ?: return null
        val address = p.getString(ADDRESS, null) ?: return null
        if (key.size != WatchWire.KEY_LEN) return null
        return Paired(key, address, p.getString(NAME, "").orEmpty())
    }

    fun save(ctx: Context, key: ByteArray, address: String, name: String) {
        prefs(ctx).edit {
            putString(KEY, Base64.getEncoder().encodeToString(key))
            putString(ADDRESS, address)
            putString(NAME, name)
        }
    }

    /** Forget the watch: its key goes, so nothing it holds fits any more. */
    fun forget(ctx: Context) {
        prefs(ctx).edit { clear() }
        // A connection already open was made with the key that is gone.
        WatchLinkServer.closeAll()
    }
}
