package dev.pocketdesk.android.notify

import android.Manifest
import android.content.Context
import android.net.Uri
import android.provider.ContactsContract

/**
 * Which contact name goes with a forwarded text (T8e), kept apart from
 * Android so it is tested.
 */
object ContactNameRule {
    /** The longest name sent, in characters (the computer keeps at most as many). */
    const val MAX_CHARS = 64

    /** A name on one trimmed line, at most [MAX_CHARS], or null when there is none. */
    fun clean(raw: String?): String? =
        raw?.split(Regex("""\s+"""))?.filter { it.isNotEmpty() }?.joinToString(" ")
            ?.take(MAX_CHARS)?.ifEmpty { null }

    /** Whether to look a sender up: the user turned names on and allowed contacts. */
    fun shouldLookUp(settingOn: Boolean, granted: Boolean): Boolean = settingOn && granted
}

/**
 * The name the phone's contacts give [address], when "Names from my
 * contacts" is on and contacts may be read; else null.
 */
fun contactName(ctx: Context, address: String): String? {
    val granted = TextPrefs.granted(ctx, Manifest.permission.READ_CONTACTS)
    if (!ContactNameRule.shouldLookUp(TextPrefs.names(ctx), granted)) return null
    val uri = Uri.withAppendedPath(ContactsContract.PhoneLookup.CONTENT_FILTER_URI, Uri.encode(address))
    val name = runCatching {
        ctx.contentResolver.query(uri, arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)
            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()
    return ContactNameRule.clean(name)
}
