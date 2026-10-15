package dev.pocketdesk.android.service

import java.security.MessageDigest

/**
 * A file's source id (T6a): 32 hex digits naming a file on this phone, the
 * same on every attempt to send it, so the computer continues what an
 * earlier attempt left. It changes when the file might have: another size
 * or another modified time is another file.
 */
object SourceId {
    fun of(uri: String, size: Long, modifiedMillis: Long?): String {
        val digest = MessageDigest.getInstance("SHA-256")
            .digest("$uri\n$size\n${modifiedMillis ?: 0}".toByteArray(Charsets.UTF_8))
        return digest.take(16).joinToString("") { "%02x".format(it) }
    }
}
