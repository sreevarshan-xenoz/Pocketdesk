package dev.pocketdesk.android.service

/**
 * Where a file from the computer goes (C2), kept apart from Android so it
 * is tested: the folder the owner picked while its grant holds, else
 * Downloads/Pocketdesk; and the names a file has while it arrives and once
 * it is whole.
 */
object DownloadPlace {
    /** What a file is called while it arrives in a picked folder. */
    const val PART_SUFFIX = ".part"

    /** The picked folder [tree] while this app still holds a write grant on it ([granted]), else null. */
    fun choose(tree: String?, granted: Collection<String>): String? =
        tree?.takeIf { it.isNotEmpty() && it in granted }

    /** Whether a folder was picked but can no longer be written: files go to Downloads, and the owner is told. */
    fun lost(tree: String?, granted: Collection<String>): Boolean =
        !tree.isNullOrEmpty() && choose(tree, granted) == null

    /** The name of [name] while it arrives. */
    fun partName(name: String): String = name + PART_SUFFIX

    /** The name of the part [part] once it is whole. */
    fun finishedName(part: String): String = part.removeSuffix(PART_SUFFIX).ifEmpty { part }
}
