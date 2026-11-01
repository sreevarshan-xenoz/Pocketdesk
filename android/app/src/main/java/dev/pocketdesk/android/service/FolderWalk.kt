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

    /** One child of a folder, as the documents provider lists it; [modified] in ms, 0 when it does not say. */
    data class Entry(
        val id: String,
        val name: String,
        val isDir: Boolean,
        val size: Long,
        val modified: Long = 0,
        /** The provider gave a row with no id, name or (when one is needed) size: counted, never silently left out. */
        val unreadable: Boolean = false,
    )

    /** One file to send. */
    data class File(val id: String, val path: List<String>, val size: Long, val modified: Long = 0)

    /**
     * What a walk is cut at. A folder kept on the computer (its own walk, on
     * a schedule) is cut far later than a folder send a person waits for.
     */
    data class Limits(val maxFiles: Int = MAX_FILES, val maxDepth: Int = MAX_DEPTH, val maxBytes: Long = MAX_BYTES)

    /**
     * What a walk found. [listFailed] counts folders the provider could not
     * list: the plan is then not the folder's whole, which only a caller that
     * deletes by it needs to know (a kept folder never infers a deletion from
     * a part).
     */
    data class Plan(val files: List<File>, val skipped: Int, val truncated: Boolean, val listFailed: Int = 0) {
        val totalBytes: Long get() = files.sumOf { it.size }

        /** Whether every folder was listed and nothing was cut short. */
        val whole: Boolean get() = listFailed == 0 && !truncated
    }

    /** Walk the folder [root], listing each folder with [children] (null: it could not be listed). */
    fun plan(root: String, children: (String) -> List<Entry>?): Plan = plan(root, Limits(), children)

    /** [plan] cut at [limits]. */
    fun plan(root: String, limits: Limits, children: (String) -> List<Entry>?): Plan {
        val files = mutableListOf<File>()
        var skipped = 0
        var truncated = false
        var listFailed = 0
        var bytes = 0L

        fun walk(id: String, under: List<String>) {
            val listed = children(id)?.sortedWith(compareBy({ !it.isDir }, { it.name.lowercase() }))
            if (listed == null) {
                skipped++
                listFailed++
                return
            }
            for (entry in listed) {
                if (truncated) return
                // A row the provider gave half: a part of the folder, not the whole.
                if (entry.unreadable) {
                    skipped++
                    listFailed++
                    continue
                }
                if (entry.name.startsWith(".")) continue
                if (!sendable(entry.name)) {
                    skipped++
                    continue
                }
                val path = under + entry.name
                if (entry.isDir) {
                    if (path.size >= limits.maxDepth) {
                        truncated = true
                        return
                    }
                    walk(entry.id, path)
                } else {
                    if (files.size >= limits.maxFiles || bytes + entry.size > limits.maxBytes) {
                        truncated = true
                        return
                    }
                    files += File(entry.id, path, entry.size, entry.modified)
                    bytes += entry.size
                }
            }
        }
        walk(root, emptyList())
        return Plan(files, skipped, truncated, listFailed)
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

    /**
     * Whether the phone makes a place the computer sent (T6b): the folder
     * [folder] and the file's [path] under it, its name last, held to the
     * computer's rules (`relpath::path_problem`: at most [MAX_DEPTH] names
     * under the folder).
     */
    fun receivable(folder: String, path: List<String>): Boolean =
        path.isNotEmpty() && path.size <= MAX_DEPTH && sendable(folder) && path.all(::sendable)

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
