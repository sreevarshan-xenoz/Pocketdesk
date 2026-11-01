package dev.pocketdesk.watchlink

/**
 * Words the computer or the phone made, as a watch may show them: short,
 * with no control or direction-changing characters (a question's text comes
 * from the computer, so it is shown plain and never trusted).
 */
object WatchText {
    /** The longest text a frame carries, in UTF-8 bytes. */
    const val MAX_BYTES = 255

    /** [text] with control and format characters taken out, cut to [maxChars] with an ellipsis. */
    fun clean(text: String, maxChars: Int): String {
        val sb = StringBuilder()
        var count = 0
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            i += Character.charCount(cp)
            val type = Character.getType(cp)
            val bad = type == Character.CONTROL.toInt() || type == Character.FORMAT.toInt() ||
                type == Character.LINE_SEPARATOR.toInt() || type == Character.PARAGRAPH_SEPARATOR.toInt() ||
                type == Character.PRIVATE_USE.toInt() || type == Character.UNASSIGNED.toInt() ||
                type == Character.SURROGATE.toInt()
            if (bad) {
                // A line break is a space; anything else invisible goes.
                if (cp == '\n'.code || cp == '\t'.code) sb.append(' ')
                continue
            }
            if (count == maxChars) {
                sb.setLength(sb.length - sb.last().let { if (Character.isLowSurrogate(it)) 2 else 1 })
                sb.append('…')
                return fit(sb.toString().trim())
            }
            sb.appendCodePoint(cp)
            count++
        }
        return fit(sb.toString().trim())
    }

    /** [text] cut at a character boundary to fit a frame's string. */
    fun fit(text: String, maxBytes: Int = MAX_BYTES): String {
        if (text.toByteArray(Charsets.UTF_8).size <= maxBytes) return text
        var end = text.length
        while (end > 0 && text.substring(0, end).toByteArray(Charsets.UTF_8).size > maxBytes) {
            end--
            if (end > 0 && Character.isLowSurrogate(text[end]) && Character.isHighSurrogate(text[end - 1])) end--
        }
        return text.substring(0, end)
    }
}
