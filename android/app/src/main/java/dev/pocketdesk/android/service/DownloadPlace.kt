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

    /** Whether [uri] names a document of a picked folder rather than a MediaStore row. */
    fun isDocument(uri: String): Boolean {
        val rest = uri.removePrefix("content://")
        if (rest == uri) return false
        val authority = rest.substringBefore('/')
        return authority != "media" && "/document/" in rest
    }

    /** The name of [name] while it arrives. */
    fun partName(name: String): String = name + PART_SUFFIX

    /** The names [name] may take in a folder, in order: itself, then `stem (1).ext`… */
    fun candidates(name: String): Sequence<String> {
        val dot = name.lastIndexOf('.').takeIf { it > 0 } ?: name.length
        val stem = name.substring(0, dot)
        val ext = name.substring(dot)
        return generateSequence(0) { it + 1 }.take(1000).map { n -> if (n == 0) name else "$stem ($n)$ext" }
    }

    /** The first of [name]'s [candidates] that neither it nor its part already takes in a folder. */
    fun freeName(name: String, taken: (String) -> Boolean): String =
        candidates(name).firstOrNull { !taken(it) && !taken(partName(it)) } ?: name

    /** The name of the part [part] once it is whole. */
    fun finishedName(part: String): String = part.removeSuffix(PART_SUFFIX).ifEmpty { part }
}
