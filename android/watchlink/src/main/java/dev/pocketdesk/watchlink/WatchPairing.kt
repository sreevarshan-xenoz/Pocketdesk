package dev.pocketdesk.watchlink

import java.security.SecureRandom

/**
 * Pairing a watch with the phone, on top of the Bluetooth bond the two
 * already have (a bond says only that the watch is the owner's, not that this
 * program on it is Pocketdesk, and any paired device could call the phone's
 * service). The owner reads six digits off the watch and types them on the
 * phone, with the phone's pairing screen open: the watch proves it knows
 * them (a MAC over the phone's challenge and its name), and the phone then
 * sends the long-term key, masked by the same digits. A wrong code counts,
 * and three close the window.
 */
object WatchPairing {
    const val CODE_DIGITS = 6

    private val TAG_PROOF = "PDW1pair".toByteArray(Charsets.US_ASCII)
    private val TAG_MASK = "PDW1mask".toByteArray(Charsets.US_ASCII)

    /** Six digits, drawn from [random]. */
    fun newCode(random: SecureRandom = SecureRandom()): String =
        (0 until CODE_DIGITS).joinToString("") { random.nextInt(10).toString() }

    /** Whether [text] is six digits (spaces taken out first). */
    fun isCode(text: String): Boolean = text.replace(" ", "").let { it.length == CODE_DIGITS && it.all { c -> c in '0'..'9' } }

    fun normalize(text: String): String = text.replace(" ", "")

    /** The watch's proof that it knows [code], for the phone's [challenge], named [watchName]. */
    fun proof(code: String, challenge: ByteArray, watchName: String): ByteArray =
        WatchWire.hmac(code.toByteArray(Charsets.US_ASCII), TAG_PROOF, challenge, watchName.toByteArray(Charsets.UTF_8))

    /** [key] xor-ed with a pad only [code] and [challenge] make; applying it twice gives [key] back. */
    fun mask(code: String, challenge: ByteArray, key: ByteArray): ByteArray {
        require(key.size == WatchWire.KEY_LEN)
        val pad = WatchWire.hmac(code.toByteArray(Charsets.US_ASCII), TAG_MASK, challenge)
        return ByteArray(key.size) { (key[it].toInt() xor pad[it].toInt()).toByte() }
    }

    /** The pair frame: tag, proof, the watch's name. */
    fun pairFrame(code: String, challenge: ByteArray, watchName: String): ByteArray {
        val name = WatchText.clean(watchName.ifBlank { "watch" }, 32)
        return byteArrayOf(WatchWire.PAIR_TAG.toByte()) + proof(code, challenge, name) +
            WatchOut().str(name).bytes()
    }

    class PairFrame(val proof: ByteArray, val name: String)

    fun readPairFrame(frame: ByteArray): PairFrame? = runCatching {
        if (frame.isEmpty() || frame[0].toInt() != WatchWire.PAIR_TAG) return null
        if (frame.size < 1 + WatchWire.MAC_LEN + 1) return null
        val proof = frame.copyOfRange(1, 1 + WatchWire.MAC_LEN)
        val r = WatchIn(frame.copyOfRange(1 + WatchWire.MAC_LEN, frame.size))
        val name = r.str()
        r.end()
        PairFrame(proof, name)
    }.getOrNull()

    /** The phone's answer to a pair frame: 0 and the masked key, or 1 for no. */
    fun accepted(maskedKey: ByteArray): ByteArray = byteArrayOf(0) + maskedKey

    fun refused(): ByteArray = byteArrayOf(1)

    /** The key in the phone's answer, unmasked with [code]; null when it was refused or is not a key. */
    fun readAnswer(code: String, challenge: ByteArray, frame: ByteArray): ByteArray? {
        if (frame.size != 1 + WatchWire.KEY_LEN || frame[0].toInt() != 0) return null
        return mask(code, challenge, frame.copyOfRange(1, frame.size))
    }

    fun newKey(random: SecureRandom = SecureRandom()): ByteArray = ByteArray(WatchWire.KEY_LEN).also(random::nextBytes)

    fun newChallenge(random: SecureRandom = SecureRandom()): ByteArray =
        ByteArray(WatchWire.CHALLENGE_LEN).also(random::nextBytes)
}

/**
 * The phone's pairing window: opened by the pairing screen, it lives two
 * minutes, takes the code the owner types, and shuts after three wrong
 * ones. The server's thread, holding the watch's connection, waits on it.
 */
class PairingWindow(private val now: () -> Long, private val lifeMs: Long = LIFE_MS) {
    private var openedAt: Long? = null
    private var code: String? = null
    private var wrong = 0

    @Synchronized
    fun open() {
        openedAt = now()
        code = null
        wrong = 0
    }

    @Synchronized
    fun close() {
        openedAt = null
        code = null
    }

    val isOpen: Boolean
        @Synchronized get() = openedAt?.let { now() - it < lifeMs } == true

    /** The owner typed [text]; false unless the window is open and it is six digits. */
    @Synchronized
    fun offer(text: String): Boolean {
        if (!isOpen || !WatchPairing.isCode(text)) return false
        code = WatchPairing.normalize(text)
        return true
    }

    /** The code typed and not yet judged, or null. */
    @Synchronized
    fun typed(): String? = if (isOpen) code else null

    /**
     * The typed code was judged: [right] ends the window with the pairing
     * made; a wrong one is forgotten, and three close the window.
     */
    @Synchronized
    fun judged(right: Boolean) {
        code = null
        if (right) {
            openedAt = null
        } else if (++wrong >= MAX_WRONG) {
            openedAt = null
        }
    }

    companion object {
        const val LIFE_MS = 120_000L
        const val MAX_WRONG = 3
    }
}
