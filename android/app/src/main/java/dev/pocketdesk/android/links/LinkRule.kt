package dev.pocketdesk.android.links

import java.net.InetAddress

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
        // A port follows the last colon; an IPv6 host keeps its colons
        // inside brackets.
        val colon = authority.lastIndexOf(':')
        val host = if (
            colon >= 0 &&
            authority.substring(colon + 1).all { it in '0'..'9' } &&
            (!authority.startsWith('[') || authority.substring(0, colon).endsWith(']'))
        ) {
            authority.substring(0, colon)
        } else {
            authority
        }
        return host.isNotEmpty() && !isLoopbackHost(host)
    }

    /**
     * Whether a browser would take [host] (lowercase) for its own machine:
     * `localhost` and its subdomains, 127/8, 0.0.0.0, `::1`, `::`, in any
     * spelling a browser reads as one of those. A host a browser rewrites
     * (percent-encoding, fullwidth characters) counts too.
     */
    private fun isLoopbackHost(raw: String): Boolean {
        val host = raw.removeSuffix(".")
        if (host == "localhost" || host.endsWith(".localhost")) return true
        if ('%' in host || host.any { it in '\uff00'..'\uffef' || it == '\u3002' || (it.code > 127 && it.isDigit()) }) {
            return true
        }
        if (host.startsWith('[') && host.endsWith(']')) {
            val inner = host.substring(1, host.length - 1)
            // A literal with a colon is parsed, never looked up.
            if (':' !in inner) return true
            val ip = runCatching { InetAddress.getByName(inner) }.getOrNull() ?: return true
            return ip.isLoopbackAddress || ip.isAnyLocalAddress
        }
        // A browser reads a host of numbers (127.1, 2130706433, 0x7f.0.0.1,
        // 0177.0.0.1) as an IPv4 address; only the plain dotted form passes.
        val parts = host.split('.')
        val numeric = parts.all { part ->
            part.isNotEmpty() && (part.all { it in '0'..'9' } || part.startsWith("0x"))
        }
        if (!numeric) return false
        val octets = parts.map { part ->
            if (part.length > 3 || (part.length > 1 && part.startsWith('0'))) return true
            part.toIntOrNull()?.takeIf { it <= 255 } ?: return true
        }
        if (octets.size != 4) return true
        return octets[0] == 127 || octets.all { it == 0 }
    }

    /** The first link in [text] that may be opened (a browser shares "title link"). */
    fun find(text: String): String? =
        text.split(Regex("""\s+""")).map { trimmed(it) }.firstOrNull { openable(it) }

    /**
     * [word] without the punctuation a sentence puts around a link: quotes,
     * brackets, a full stop. A closing bracket the link itself opened stays
     * (`…/Foo_(bar)`).
     */
    private fun trimmed(word: String): String {
        var link = word.trimStart('(', '<', '[', '"', '\'')
        while (link.isNotEmpty()) {
            val last = link.last()
            val strip = when (last) {
                '.', ',', ';', ':', '!', '?', '"', '\'', '>' -> true
                ')' -> link.count { it == '(' } < link.count { it == ')' }
                ']' -> link.count { it == '[' } < link.count { it == ']' }
                else -> false
            }
            if (!strip) break
            link = link.dropLast(1)
        }
        return link
    }
}
