package dev.linuxlink.android.ui

import java.util.Locale

/** One entry of a folder on the computer (F7b). [path] is the computer's own, used as is. */
data class RemoteEntry(
    val name: String,
    val path: String,
    val isDirectory: Boolean,
    val size: Long,
    val modified: Long,
)

/**
 * A folder on the computer as its browse reply describes it: where it is,
 * the way up (`null` at the home folder), its entries, or why it could not
 * be listed.
 */
data class RemoteFolder(
    val path: String,
    val parent: String?,
    val entries: List<RemoteEntry>,
    val error: String?,
)

/** What the Files sheet shows, kept apart from Android so it is tested. */
object RemoteFiles {
    /** Folders first, then by name ignoring case. */
    fun sorted(entries: List<RemoteEntry>): List<RemoteEntry> =
        entries.sortedWith(compareBy<RemoteEntry>({ !it.isDirectory }, { it.name.lowercase(Locale.ROOT) }))

    /**
     * Where Up goes when [failedPath] could not be listed at all: back to the
     * last folder that loaded, or that folder's parent when it is the one
     * that failed again. `null` when nothing has loaded.
     */
    fun wayBack(failedPath: String, lastGood: RemoteFolder?): String? = when {
        lastGood == null -> null
        lastGood.path != failedPath -> lastGood.path
        else -> lastGood.parent
    }

    /** The folder's own name, whichever separator the computer uses. */
    fun title(path: String): String = path.trimEnd('/', '\\').substringAfterLast('/').substringAfterLast('\\')

    /** `2_100_000` -> "2.0 MB". */
    fun humanSize(bytes: Long): String {
        if (bytes < 1024) return "$bytes B"
        val units = listOf("KB", "MB", "GB", "TB")
        var value = bytes / 1024.0
        var unit = 0
        while (value >= 1024 && unit < units.size - 1) {
            value /= 1024
            unit++
        }
        return String.format(Locale.ROOT, "%.1f %s", value, units[unit])
    }
}
