package dev.pocketdesk.android

/**
 * A computer's name on this phone (U3): what it calls itself (in the pairing
 * link and on each connection) and the owner's rename, kept per address.
 */
object ComputerName {
    /** The longest name kept, in characters, as the computer keeps a phone's. */
    const val MAX_CHARS = 40

    /** Characters that change how text reads without being seen (the core's list). */
    private fun invisible(cp: Int): Boolean =
        cp == 0x061C || cp in 0x200B..0x200F || cp in 0x202A..0x202E ||
            cp in 0x2060..0x2069 || cp == 0xFEFF

    /**
     * [raw] as one short printable line: control and invisible characters
     * dropped, white space one space, at most [MAX_CHARS]; null when nothing
     * is left. The core's `clean_device_name`, for names this side takes in.
     */
    fun clean(raw: String?): String? {
        if (raw == null) return null
        val out = StringBuilder()
        var count = 0
        var space = false
        var i = 0
        while (i < raw.length && count < MAX_CHARS) {
            val cp = raw.codePointAt(i)
            i += Character.charCount(cp)
            when {
                Character.isWhitespace(cp) || Character.isSpaceChar(cp) -> space = out.isNotEmpty()
                Character.isISOControl(cp) || invisible(cp) -> {}
                else -> {
                    if (space) {
                        out.append(' ')
                        count++
                        space = false
                        if (count >= MAX_CHARS) break
                    }
                    out.appendCodePoint(cp)
                    count++
                }
            }
        }
        return out.toString().trimEnd().ifEmpty { null }
    }

    /** What a computer is shown by: the owner's name, else its own, else its address. */
    fun shown(chosen: String?, claimed: String?, address: String): String =
        chosen ?: claimed ?: address
}
