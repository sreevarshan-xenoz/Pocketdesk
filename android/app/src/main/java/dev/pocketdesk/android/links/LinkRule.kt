package dev.pocketdesk.android.links

/**
 * Which links may go between the phone and the computer (T8a), the same
 * rules as the computer's `core::protocol::link::link_problem`: an `http` or
 * `https` link of at most [MAX_BYTES], with a host, no user name or password,
 * and no space or control character. Anything else (`file:`, `javascript:`,
 * an `intent:`) could do more than show a page. Kept apart from Android so
 * it is tested.
 */
object LinkRule {
    const val MAX_BYTES = 2048

    fun openable(url: String): Boolean {
        if (url.toByteArray(Charsets.UTF_8).size > MAX_BYTES) return false
        if (url.any { it.isWhitespace() || it.isISOControl() }) return false
        val lower = url.lowercase()
        val rest = when {
            lower.startsWith("https://") -> lower.removePrefix("https://")
            lower.startsWith("http://") -> lower.removePrefix("http://")
            else -> return false
        }
        val authority = rest.split('/', '?', '#').first()
        if ('@' in authority) return false
        val colon = authority.lastIndexOf(':')
        val host = if (colon > 0 && authority.substring(colon + 1).all { it.isDigit() }) {
            authority.substring(0, colon)
        } else {
            authority
        }
        return host.isNotEmpty()
    }

    /** The first link in [text] that may be opened (a browser shares "title link"). */
    fun find(text: String): String? = text.split(Regex("""\s+""")).firstOrNull { openable(it) }
}
