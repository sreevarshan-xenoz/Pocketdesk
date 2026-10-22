package dev.pocketdesk.android.files

/**
 * What the computer may add to this phone's shared folders (N6): only from
 * its own computer, only into a folder its owner marked, only under names a
 * listing shows (none starting with a dot). Kept apart from Android so it is
 * tested.
 */
object AddRule {
    enum class Decision { ADD, NOT_OWN_COMPUTER, NOT_MARKED, BAD_NAME }

    /** What the computer hears for a folder that takes nothing. */
    const val NOT_MARKED = "The phone does not let the computer add files there"

    /** What the computer hears for a name the phone does not take. */
    const val BAD_NAME = "That name cannot be added on the phone"

    /** `core::protocol::phone_files::EXISTS`, word for word: the computer reads it. */
    const val EXISTS = "There is a folder or file of that name already"

    /**
     * Whether an addition into [path] (the folders below the root), named
     * [name] (null for a folder, whose name ends [path]), may be made.
     */
    fun decide(ownComputer: Boolean, marked: Boolean, path: List<String>, name: String?): Decision = when {
        !ownComputer -> Decision.NOT_OWN_COMPUTER
        !marked -> Decision.NOT_MARKED
        path.size > PhoneFilesRule.MAX_DEPTH || !path.all(PhoneFilesRule::listed) -> Decision.BAD_NAME
        name == null && path.isEmpty() -> Decision.BAD_NAME
        name != null && !PhoneFilesRule.listed(name) -> Decision.BAD_NAME
        else -> Decision.ADD
    }

    /** What the computer hears for [decision], when it is told (never for another computer). */
    fun reason(decision: Decision): String? = when (decision) {
        Decision.NOT_MARKED -> NOT_MARKED
        Decision.BAD_NAME -> BAD_NAME
        else -> null
    }
}
