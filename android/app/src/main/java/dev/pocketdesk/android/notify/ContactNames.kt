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

    /**
     * A name on one trimmed line, at most [MAX_CHARS] characters (whole
     * ones: an emoji at the cut is not split), or null when there is none.
     */
    fun clean(raw: String?): String? {
        val line = raw?.split(Regex("""\s+"""))?.filter { it.isNotEmpty() }?.joinToString(" ") ?: return null
        if (line.isEmpty()) return null
        val chars = line.codePointCount(0, line.length)
        return if (chars <= MAX_CHARS) line else line.substring(0, line.offsetByCodePoints(0, MAX_CHARS))
    }

    /**
     * Whether to look [address] up: the user turned names on and allowed
     * contacts, and it is a number (a sender ID with letters would have its
     * keypad digits matched to an unrelated contact).
     */
    fun shouldLookUp(settingOn: Boolean, granted: Boolean, address: String): Boolean =
        settingOn && granted && address.none { it.isLetter() }
}

/**
 * The name the phone's contacts give [address], when "Names from my
 * contacts" is on and contacts may be read; else null. It queries the
 * contacts provider: call it off the main thread.
 */
fun contactName(ctx: Context, address: String): String? {
    val granted = TextPrefs.granted(ctx, Manifest.permission.READ_CONTACTS)
    if (!ContactNameRule.shouldLookUp(TextPrefs.names(ctx), granted, address)) return null
    val uri = Uri.withAppendedPath(ContactsContract.PhoneLookup.CONTENT_FILTER_URI, Uri.encode(address))
    val name = runCatching {
        ctx.contentResolver.query(uri, arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)
            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()
    return ContactNameRule.clean(name)
}
