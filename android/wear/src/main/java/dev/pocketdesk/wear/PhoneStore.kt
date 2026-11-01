package dev.pocketdesk.wear

import android.content.Context
import androidx.core.content.edit
import dev.pocketdesk.watchlink.WatchWire
import java.util.Base64

/**
 * The phone this watch is paired with: its Bluetooth address and the key it
 * made for this watch. Private to the app and left out of every backup and
 * transfer; never logged and never shown.
 */
object PhoneStore {
    private const val PREFS = "pocketdesk-phone"
    private const val KEY = "key"
    private const val ADDRESS = "address"

    class Paired(val key: ByteArray, val address: String)

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun paired(ctx: Context): Paired? {
        val p = prefs(ctx)
        val key = p.getString(KEY, null)?.let { runCatching { Base64.getDecoder().decode(it) }.getOrNull() } ?: return null
        val address = p.getString(ADDRESS, null) ?: return null
        return if (key.size == WatchWire.KEY_LEN) Paired(key, address) else null
    }

    fun save(ctx: Context, key: ByteArray, address: String) {
        prefs(ctx).edit {
            putString(KEY, Base64.getEncoder().encodeToString(key))
            putString(ADDRESS, address)
        }
    }

    fun forget(ctx: Context) {
        prefs(ctx).edit { clear() }
    }
}
