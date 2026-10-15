package dev.pocketdesk.android.service

/**
 * What a folder send (T6b) sends: every file under a folder the person
 * picked, with its path under it as a list of names, in a stable order.
 *
 * Hidden files and folders (a leading dot) are left out, as are names the
 * computer would refuse (the same rules as `core::protocol::relpath`):
 * they are counted, so the person is told, as is a folder the provider
 * could not list (never taken for an empty one). A folder deeper than
 * [MAX_DEPTH], or past [MAX_FILES] files or [MAX_BYTES], is cut short.
 */
object FolderWalk {
    const val MAX_FILES = 10_000
    const val MAX_DEPTH = 32
    const val MAX_BYTES = 32L * 1024 * 1024 * 1024

    /** One child of a folder, as the documents provider lists it. */
    data class Entry(val id: String, val name: String, val isDir: Boolean, val size: Long)

    /** One file to send. */
    data class File(val id: String, val path: List<String>, val size: Long)

    data class Plan(val files: List<File>, val skipped: Int, val truncated: Boolean) {
        val totalBytes: Long get() = files.sumOf { it.size }
    }

    /** Walk the folder [root], listing each folder with [children] (null: it could not be listed). */
    fun plan(root: String, children: (String) -> List<Entry>?): Plan {
        val files = mutableListOf<File>()
        var skipped = 0
        var truncated = false
        var bytes = 0L

        fun walk(id: String, under: List<String>) {
            val listed = children(id)?.sortedWith(compareBy({ !it.isDir }, { it.name.lowercase() }))
            if (listed == null) {
                skipped++
                return
            }
            for (entry in listed) {
                if (truncated) return
                if (entry.name.startsWith(".")) continue
                if (!sendable(entry.name)) {
                    skipped++
                    continue
                }
                val path = under + entry.name
                if (entry.isDir) {
                    if (path.size >= MAX_DEPTH) {
                        truncated = true
                        return
                    }
                    walk(entry.id, path)
                } else {
                    if (files.size >= MAX_FILES || bytes + entry.size > MAX_BYTES) {
                        truncated = true
                        return
                    }
                    files += File(entry.id, path, entry.size)
                    bytes += entry.size
                }
            }
        }
        walk(root, emptyList())
        return Plan(files, skipped, truncated)
    }

    /**
     * A name for a folder received from the computer (T6b) that no earlier
     * one has: [base], else `base (1)`, `base (2)`… so a send never merges
     * into a folder that is already there.
     */
    fun freeName(base: String, taken: (String) -> Boolean): String =
        generateSequence(0) { it + 1 }
            .map { n -> if (n == 0) base else "$base ($n)" }
            .take(1000)
            .firstOrNull { !taken(it) } ?: "$base (${System.currentTimeMillis()})"

    /**
     * The characters that change how a name reads without being seen
     * (`kdeconnect::is_invisible_format` on the computer): bidi controls and
     * zero-width ones.
     */
    private fun isInvisibleFormat(c: Char): Boolean =
        c == '\u061C' || c in '\u200B'..'\u200F' || c in '\u202A'..'\u202E' ||
            c in '\u2060'..'\u2069' || c == '\uFEFF'

    /** Whether the computer takes [name] as one name in a path. */
    fun sendable(name: String): Boolean {
        if (name.isEmpty() || name == "." || name == "..") return false
        if (name.toByteArray(Charsets.UTF_8).size > 255) return false
        if (name.any { it.isISOControl() || it in "/\\:<>\"|?*" }) return false
        if (name.any(::isInvisibleFormat)) return false
        if (name.endsWith(" ") || name.endsWith(".")) return false
        val stem = name.substringBefore('.').trimEnd().uppercase()
        if (stem in setOf("CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$")) return false
        if (stem.length == 4 && (stem.startsWith("COM") || stem.startsWith("LPT")) && stem[3] in "0123456789¹²³") {
            return false
        }
        return true
    }
}
