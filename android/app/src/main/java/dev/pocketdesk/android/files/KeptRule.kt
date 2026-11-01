package dev.pocketdesk.android.files

import dev.pocketdesk.android.service.FolderWalk

/**
 * The decisions of "Folders kept on my computer" that touch no Android: what
 * a run sends, what it may tell the computer to delete, and when the
 * schedule lets it start. One-way: the phone is the source, and the
 * computer's copy is a mirror of it that never deletes unless both ends say
 * so.
 */
object KeptRule {
    /** What a file was when it last reached the computer. */
    data class Mark(val size: Long, val modified: Long)

    /** A file's key in a record: its names joined; a name never holds a slash. */
    fun key(path: List<String>): String = path.joinToString("/")

    /** The names a [key] was made of. */
    fun path(key: String): List<String> = key.split('/')

    /** What a run does for one folder. */
    data class Diff(
        /** The files to send: new, or changed in size or modified time. */
        val send: List<FolderWalk.File>,
        /** Files the phone had sent and no longer has: deleted on the phone. */
        val gone: List<String>,
        /** Why the deletions were held back, if they were. */
        val held: Held?,
    )

    enum class Held {
        /** A part of the folder was listed, not the whole: nothing is known to be gone. */
        PART,

        /** Most of what was kept is gone at once: the folder was emptied or its storage went away. */
        MANY,
    }

    /**
     * Deletions are held when more than half of what was kept is gone at
     * once, whatever the folder's size, unless this few ([FREE_GONE]) are
     * gone: one file deleted from a folder of two still mirrors. A listing
     * with no file in it while the record holds some is always held (an
     * emptied folder looks like a storage that went away), so 1 of 1 is
     * mirrored only when the listing is otherwise healthy.
     */
    const val FREE_GONE = 1

    /**
     * The files [plan] holds that [record] does not hold as they are, and
     * the files [record] holds that [plan] no longer does. Deletions are
     * only named when the plan is the folder's whole, and not when nearly
     * everything is gone at once: a provider that failed, or an SD card
     * taken out, must never empty the computer's copy.
     */
    fun diff(plan: FolderWalk.Plan, record: Map<String, Mark>): Diff {
        val send = plan.files.filter { file ->
            val mark = record[key(file.path)]
            mark == null || mark.size != file.size || mark.modified != file.modified
        }
        val present = plan.files.map { key(it.path) }.toHashSet()
        val gone = record.keys.filter { it !in present }.sorted()
        val held = when {
            gone.isEmpty() -> null
            !plan.whole -> Held.PART
            plan.files.isEmpty() -> Held.MANY
            gone.size > FREE_GONE && gone.size * 2 > record.size -> Held.MANY
            else -> null
        }
        return Diff(send, if (held == null) gone else emptyList(), held)
    }

    /** A record's text: one line a file, `size`, `modified` and the key, tab apart. */
    fun write(record: Map<String, Mark>): String =
        record.entries.sortedBy { it.key }
            .joinToString("") { (key, mark) -> "${mark.size}\t${mark.modified}\t$key\n" }

    /** [write]'s text, read; a line that is not one is dropped. */
    fun read(text: String): Map<String, Mark> {
        val record = LinkedHashMap<String, Mark>()
        for (line in text.lineSequence()) {
            val parts = line.split('\t', limit = 3)
            if (parts.size != 3 || parts[2].isEmpty()) continue
            val size = parts[0].toLongOrNull() ?: continue
            val modified = parts[1].toLongOrNull() ?: continue
            record[parts[2]] = Mark(size, modified)
        }
        return record
    }

    /** When the schedule starts a run. */
    enum class Mode {
        /** On Wi-Fi and charging only (the default). */
        WIFI_CHARGING,

        /** Every few hours, on Wi-Fi, charging or not. */
        EVERY,
    }

    /** The hours a run recurs by, and the choices Settings offers for [Mode.EVERY]. */
    val HOURS = listOf(1, 3, 6, 12, 24)
    const val DEFAULT_HOURS = 6

    /** How often the job comes round: [hours] for [Mode.EVERY], else the default. */
    fun periodHours(mode: Mode, hours: Int): Int =
        if (mode == Mode.EVERY && hours in HOURS) hours else DEFAULT_HOURS

    /** What the system must hold before a run starts: always an unmetered network, and charging by default. */
    data class Needs(val unmetered: Boolean, val charging: Boolean)

    fun needs(mode: Mode): Needs = Needs(unmetered = true, charging = mode == Mode.WIFI_CHARGING)

    /** Whether a run may start now: what it needs, held. */
    fun mayStart(mode: Mode, unmetered: Boolean, charging: Boolean): Boolean {
        val needs = needs(mode)
        return (!needs.unmetered || unmetered) && (!needs.charging || charging)
    }

    /** A folder's name for the list: what the picker's document id ends in, else [fallback]. */
    fun label(documentId: String?, fallback: String): String =
        documentId?.substringAfterLast(':')?.substringAfterLast('/')?.trim()?.ifEmpty { null }
            ?.take(80) ?: fallback

    /** The computer's refusal because the kept folders hold what its cap allows. */
    const val FULL = "the kept folders on this computer are full"

    /** Whether a send's [error] says the computer's cap is reached: the whole run stops. */
    fun isFull(error: String?): Boolean = error?.contains(FULL, ignoreCase = true) == true

    /** The computer's refusal because it keeps no folders for this phone. */
    fun isNotKept(error: String?): Boolean =
        error != null && listOf("turned off on this computer", "take only the owner").any { error.contains(it, ignoreCase = true) }

    /** The most paths one run-end packet names (the computer's own limit). */
    const val MAX_GONE_PER_PACKET = 200

    /** A folder's id: 32 hex digits, as the computer expects. */
    fun validId(id: String): Boolean = id.length == 32 && id.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }
}
