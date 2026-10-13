package dev.linuxlink.android.service

/**
 * The name a file from the computer is saved under (F7b). The computer
 * chooses the text, so only its last path part is kept, without control
 * characters, never hidden, and short.
 */
object DownloadName {
    const val MAX_LENGTH = 120
    private const val FALLBACK = "download"

    fun safe(raw: String): String {
        val last = raw.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = last.filterNot { it.isISOControl() }.trim().trimStart('.').trimEnd('.', ' ')
        if (cleaned.isBlank()) return FALLBACK
        if (cleaned.length <= MAX_LENGTH) return cleaned
        val ext = extension(cleaned).let { if (it.isNotEmpty() && it.length <= 15) ".$it" else "" }
        return cleaned.take(MAX_LENGTH - ext.length).trimEnd('.', ' ') + ext
    }

    /** The extension after the last dot, lower case; empty when there is none. */
    fun extension(name: String): String {
        val dot = name.lastIndexOf('.')
        return if (dot > 0 && dot < name.length - 1) name.substring(dot + 1).lowercase() else ""
    }
}
