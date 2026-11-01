package dev.pocketdesk.watchlink

import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PublicKey
import java.security.SecureRandom
import java.security.spec.ECGenParameterSpec
import java.security.spec.X509EncodedKeySpec
import javax.crypto.KeyAgreement

/**
 * Pairing a watch with the phone, on top of the Bluetooth bond the two
 * already have (a bond says only that the watch is the owner's, not that this
 * program on it is Pocketdesk, and any paired device could call the phone's
 * service). The owner reads six digits off the watch and types them on the
 * phone, with the phone's pairing screen open: the watch proves it knows
 * them (a MAC over the phone's challenge, its name and its ephemeral key),
 * and the phone then sends the long-term key masked by a pad made from the
 * digits **and** an ephemeral ECDH secret.
 *
 * Six digits are about 20 bits, so the digits alone would not protect the
 * key from someone who recorded the exchange: the Bluetooth bond's encryption
 * is what keeps the exchange from being recorded, and the ECDH secret means
 * that even a recording of it, with the digits guessed, does not give the
 * key. (The devices' Bluetooth addresses are not bound in: Android hides an
 * app's own device address, so the two sides cannot both name both, and the
 * addresses cross the air in the clear anyway.) A wrong code is counted per
 * Bluetooth address ([PairingWindow]).
 */
object WatchPairing {
    const val CODE_DIGITS = 6

    private val TAG_PROOF = "PDW1pair2".toByteArray(Charsets.US_ASCII)
    private val TAG_MASK = "PDW1mask2".toByteArray(Charsets.US_ASCII)

    /** Six digits, drawn from [random]. */
    fun newCode(random: SecureRandom = SecureRandom()): String =
        (0 until CODE_DIGITS).joinToString("") { random.nextInt(10).toString() }

    /** Whether [text] is six digits (spaces taken out first). */
    fun isCode(text: String): Boolean = text.replace(" ", "").let { it.length == CODE_DIGITS && it.all { c -> c in '0'..'9' } }

    fun normalize(text: String): String = text.replace(" ", "")

    /** A fresh P-256 pair for one pairing; never kept. */
    fun newEphemeral(): KeyPair =
        KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()

    /** The ECDH secret between [mine] and the other side's public key [theirs] (X.509); null if [theirs] is not a P-256 key. */
    fun agree(mine: KeyPair, theirs: ByteArray): ByteArray? = runCatching {
        val key: PublicKey = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(theirs))
        KeyAgreement.getInstance("ECDH").run {
            init(mine.private)
            doPhase(key, true)
            generateSecret()
        }
    }.getOrNull()

    /** The watch's proof that it knows [code], for the phone's [challenge], named [watchName], with its ephemeral key [watchPublic]. */
    fun proof(code: String, challenge: ByteArray, watchName: String, watchPublic: ByteArray): ByteArray =
        WatchWire.hmac(code.toByteArray(Charsets.US_ASCII), TAG_PROOF, challenge, watchName.toByteArray(Charsets.UTF_8), watchPublic)

    /** [key] xor-ed with a pad only [code], [challenge] and the ECDH [secret] make; applying it twice gives [key] back. */
    fun mask(code: String, challenge: ByteArray, secret: ByteArray, key: ByteArray): ByteArray {
        require(key.size == WatchWire.KEY_LEN)
        val pad = WatchWire.hmac(code.toByteArray(Charsets.US_ASCII), TAG_MASK, challenge, secret)
        return ByteArray(key.size) { (key[it].toInt() xor pad[it].toInt()).toByte() }
    }

    /** The pair frame: tag, proof, the watch's name, its ephemeral public key. */
    fun pairFrame(code: String, challenge: ByteArray, watchName: String, ephemeral: KeyPair): ByteArray {
        val name = WatchText.clean(watchName.ifBlank { "watch" }, 32)
        val pub = ephemeral.public.encoded
        return byteArrayOf(WatchWire.PAIR_TAG.toByte()) + proof(code, challenge, name, pub) +
            WatchOut().str(name).blob(pub).bytes()
    }

    class PairFrame(val proof: ByteArray, val name: String, val publicKey: ByteArray)

    fun readPairFrame(frame: ByteArray): PairFrame? {
        if (frame.size < 1 + WatchWire.MAC_LEN + 2 || frame[0].toInt() != WatchWire.PAIR_TAG) return null
        return runCatching {
            val proof = frame.copyOfRange(1, 1 + WatchWire.MAC_LEN)
            val r = WatchIn(frame.copyOfRange(1 + WatchWire.MAC_LEN, frame.size))
            val name = r.str()
            val pub = r.blob()
            r.end()
            PairFrame(proof, name, pub)
        }.getOrNull()
    }

    /** The phone's answer to a pair frame: 0, its ephemeral public key, the masked key; or 1 for no. */
    fun accepted(phonePublic: ByteArray, maskedKey: ByteArray): ByteArray =
        byteArrayOf(0) + WatchOut().blob(phonePublic).bytes() + maskedKey

    fun refused(): ByteArray = byteArrayOf(1)

    /** The key in the phone's answer, unmasked with [code] and [mine]; null when it was refused or is not a key. */
    fun readAnswer(code: String, challenge: ByteArray, mine: KeyPair, frame: ByteArray): ByteArray? {
        if (frame.isEmpty() || frame[0].toInt() != 0) return null
        return runCatching {
            val r = WatchIn(frame.copyOfRange(1, frame.size))
            val pub = r.blob()
            val rest = frame.copyOfRange(1 + 1 + pub.size, frame.size)
            if (rest.size != WatchWire.KEY_LEN) return null
            val secret = agree(mine, pub) ?: return null
            mask(code, challenge, secret, rest)
        }.getOrNull()
    }

    fun newKey(random: SecureRandom = SecureRandom()): ByteArray = ByteArray(WatchWire.KEY_LEN).also(random::nextBytes)

    fun newChallenge(random: SecureRandom = SecureRandom()): ByteArray =
        ByteArray(WatchWire.CHALLENGE_LEN).also(random::nextBytes)
}

/**
 * The phone's pairing window: opened by the pairing screen, it lives two
 * minutes and takes the code the owner types. Each connection judges its own
 * proof against that code, and a wrong one is counted against the Bluetooth
 * address that made it, so a rogue bonded device cannot use up the owner's
 * tries: it is shut out after [MAX_WRONG] of its own, and the window closes
 * for everyone after [MAX_WRONG_TOTAL] wrong in all. A right one closes it.
 */
class PairingWindow(private val now: () -> Long, private val lifeMs: Long = LIFE_MS) {
    private var openedAt: Long? = null
    private var code: String? = null
    private val wrong = HashMap<String, Int>()
    private val pending = HashSet<String>()
    private var wrongTotal = 0

    @Synchronized
    fun open() {
        openedAt = now()
        code = null
        wrong.clear()
        pending.clear()
        wrongTotal = 0
    }

    @Synchronized
    fun close() {
        openedAt = null
        code = null
    }

    val isOpen: Boolean
        @Synchronized get() = openedAt?.let { now() - it < lifeMs } == true && wrongTotal < MAX_WRONG_TOTAL

    /** The owner typed [text]; false unless the window is open and it is six digits. */
    @Synchronized
    fun offer(text: String): Boolean {
        if (!isOpen || !WatchPairing.isCode(text)) return false
        code = WatchPairing.normalize(text)
        return true
    }

    /** The code typed, or null. */
    @Synchronized
    fun typed(): String? = if (isOpen) code else null

    /**
     * Whether a connection from [address] may wait to pair: the window is
     * open, the address has tries left and has no other connection waiting.
     * Pair it with [release] when it ends.
     */
    @Synchronized
    fun enter(address: String): Boolean {
        if (!isOpen || (wrong[address] ?: 0) >= MAX_WRONG) return false
        return pending.add(address)
    }

    @Synchronized
    fun release(address: String) {
        pending.remove(address)
    }

    /** The connection from [address] was judged: [right] ends the window; a wrong one counts against that address. */
    @Synchronized
    fun judged(address: String, right: Boolean) {
        if (right) {
            openedAt = null
            code = null
        } else {
            wrong[address] = (wrong[address] ?: 0) + 1
            wrongTotal++
        }
    }

    companion object {
        const val LIFE_MS = 120_000L

        /** Wrong codes one Bluetooth address may try in a window. */
        const val MAX_WRONG = 3

        /** Wrong codes in all, whoever, before the window closes. */
        const val MAX_WRONG_TOTAL = 10
    }
}
