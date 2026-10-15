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

    /** Whether [name] is one part of a path: no separator, never `.` or `..`, no control character. */
    fun validName(name: String): Boolean =
        name.isNotEmpty() && name != "." && name != ".." && name.length <= MAX_NAME &&
            name.none { it == '/' || it == '\\' || it.isISOControl() }

    fun validPath(path: List<String>): Boolean = path.size <= MAX_DEPTH && path.all(::validName)

    /** Whether an entry is listed: a usable name that is not hidden. */
    fun listed(name: String): Boolean = validName(name) && !name.startsWith(".")

    fun fromOwnComputer(host: String, port: Int, ownAddress: String?, ownPort: Int?): Boolean =
        ownAddress != null && host == ownAddress && port == ownPort

    fun root(folders: List<PhoneFolders.Folder>, id: Int): PhoneFolders.Folder? = folders.firstOrNull { it.id == id }

    /** The id a newly shared folder takes: never one used before in the list. */
    fun nextId(folders: List<PhoneFolders.Folder>): Int = (folders.maxOfOrNull { it.id } ?: -1) + 1
}
