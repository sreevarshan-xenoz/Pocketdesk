package dev.pocketdesk.android.files

/**
 * The choices of browsing this phone's shared folders from the computer
 * (T10g), held to the same rules as the computer's side
 * (`core::protocol::phone_files`). Pure, for JVM tests.
 */
object PhoneFilesRule {
    /** Names a path holds below its root, at most. */
    const val MAX_DEPTH = 32

    /** Entries one listing carries, at most. */
    const val MAX_ENTRIES = 2000

    private const val MAX_NAME = 255

    /**
     * Whether [name] is one part of a path: no separator, never `.` or `..`,
     * no control or invisible format character (`kdeconnect::is_invisible_format`).
     */
    fun validName(name: String): Boolean =
        name.isNotEmpty() && name != "." && name != ".." && name.codePointCount(0, name.length) <= MAX_NAME &&
            name.none(::refused)

    private fun refused(c: Char): Boolean =
        c == '/' || c == '\\' || c.isISOControl() || c == '\u061C' || c in '\u200B'..'\u200F' ||
            c in '\u202A'..'\u202E' || c in '\u2060'..'\u2069' || c == '\uFEFF'

    /**
     * The name a shared folder goes by on the computer: [raw] when the
     * computer takes it, else with what it refuses taken out, else [fallback].
     */
    fun rootName(raw: String, fallback: String): String {
        if (validName(raw)) return raw
        val cleaned = raw.filterNot(::refused).trim()
        val cut =
            if (cleaned.codePointCount(0, cleaned.length) > MAX_NAME) {
                cleaned.substring(0, cleaned.offsetByCodePoints(0, MAX_NAME))
            } else {
                cleaned
            }
        return if (validName(cut)) cut else fallback
    }

    fun validPath(path: List<String>): Boolean = path.size <= MAX_DEPTH && path.all(::validName)

    /** Whether an entry is listed: a usable name that is not hidden. */
    fun listed(name: String): Boolean = validName(name) && !name.startsWith(".")

    fun fromOwnComputer(host: String, port: Int, ownAddress: String?, ownPort: Int?): Boolean =
        ownAddress != null && host == ownAddress && port == ownPort

    fun root(folders: List<PhoneFolders.Folder>, id: Int): PhoneFolders.Folder? = folders.firstOrNull { it.id == id }

    /** The id a newly shared folder takes: never one used before in the list. */
    fun nextId(folders: List<PhoneFolders.Folder>): Int = (folders.maxOfOrNull { it.id } ?: -1) + 1
}
