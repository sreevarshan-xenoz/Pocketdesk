package dev.pocketdesk.android.clip

/**
 * O7: whether a file the computer sent goes onto this phone's clipboard
 * too: with Settings "Images from the computer onto my clipboard" on, a PNG
 * or a JPEG (by its first bytes, never its name) of at most [MAX_BYTES],
 * sent on its own. Pure, for JVM tests.
 */
object ReceivedImageRule {
    /** The largest file put on the clipboard, as N4's images. */
    const val MAX_BYTES = 32L shl 20

    /** The bytes a file's kind is read from. */
    const val HEAD = 8

    enum class Kind(val extension: String) { PNG("png"), JPEG("jpg") }

    private val PNG_SIGNATURE = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 0x0D, 0x0A, 0x1A, 0x0A)

    /** The kind of image [head] starts, or null for anything else. */
    fun kind(head: ByteArray): Kind? = when {
        head.size >= PNG_SIGNATURE.size && head.copyOfRange(0, PNG_SIGNATURE.size).contentEquals(PNG_SIGNATURE) -> Kind.PNG
        head.size >= 3 && head[0] == 0xFF.toByte() && head[1] == 0xD8.toByte() && head[2] == 0xFF.toByte() -> Kind.JPEG
        else -> null
    }

    /**
     * Whether a file of [size] bytes goes: the switch [enabled], sent
     * [onItsOwn] (not one of a folder's files, nor into a shared folder of
     * N6's), not empty and at most [MAX_BYTES].
     */
    fun takes(enabled: Boolean, onItsOwn: Boolean, size: Long): Boolean =
        enabled && onItsOwn && size in 1..MAX_BYTES
}
