package dev.pocketdesk.watchlink

import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.UUID
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * The frames between a watch and the phone it is paired with, over a
 * Bluetooth stream, with no Google service in the way.
 *
 * The phone speaks first: [MAGIC], then a fresh random challenge, and
 * whether it holds a key for this watch. A request is its op, a counter and
 * a body, with a MAC (HMAC-SHA256 under the key made at pairing) over the
 * challenge, the op, the counter and the body: another program on a paired
 * Bluetooth device cannot make one, one recorded earlier does not fit a new
 * challenge, and one replayed in the same connection does not beat the
 * counter. The answer carries a MAC over the same challenge and counter.
 * Nothing here is secret but the key; the link itself is encrypted by the
 * Bluetooth bond the watch and phone already have.
 */
object WatchWire {
    const val MAGIC = "PDW1"
    const val CHALLENGE_LEN = 16
    const val KEY_LEN = 32
    const val MAC_LEN = 32

    /** The longest frame either side reads: a frame is never trusted for its own size. */
    const val MAX_FRAME = 2048

    /** The service the phone listens on and the watch calls (a random id). */
    val SERVICE: UUID = UUID.fromString("6f1c3f0a-5d1e-4b7a-9a3e-7c2f4d8e1b52")

    /** The service's name, which Bluetooth shows. */
    const val SERVICE_NAME = "Pocketdesk watch link"

    /** The first byte of a frame that is not a request but the pairing's. */
    const val PAIR_TAG = 0

    private val TAG_REQUEST = "PDW1q".toByteArray(Charsets.US_ASCII)
    private val TAG_RESPONSE = "PDW1r".toByteArray(Charsets.US_ASCII)

    fun hmac(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        parts.forEach(mac::update)
        return mac.doFinal()
    }

    /** Whether two MACs are equal, in the time of the length alone. */
    fun same(a: ByteArray, b: ByteArray): Boolean = MessageDigest.isEqual(a, b)

    /** The phone's first frame. */
    fun hello(challenge: ByteArray, paired: Boolean): ByteArray {
        require(challenge.size == CHALLENGE_LEN)
        return MAGIC.toByteArray(Charsets.US_ASCII) + challenge + byteArrayOf(if (paired) 1 else 0)
    }

    class Hello(val challenge: ByteArray, val paired: Boolean)

    fun readHello(frame: ByteArray): Hello? {
        val magic = MAGIC.toByteArray(Charsets.US_ASCII)
        if (frame.size != magic.size + CHALLENGE_LEN + 1) return null
        if (!frame.copyOfRange(0, magic.size).contentEquals(magic)) return null
        val flag = frame[frame.size - 1].toInt()
        if (flag != 0 && flag != 1) return null
        return Hello(frame.copyOfRange(magic.size, magic.size + CHALLENGE_LEN), flag == 1)
    }

    class Request(val op: WatchOp, val counter: Long, val body: ByteArray)

    private fun u32(n: Long): ByteArray =
        byteArrayOf((n shr 24).toByte(), (n shr 16).toByte(), (n shr 8).toByte(), n.toByte())

    private fun readU32(b: ByteArray, at: Int): Long =
        ((b[at].toLong() and 0xff) shl 24) or ((b[at + 1].toLong() and 0xff) shl 16) or
            ((b[at + 2].toLong() and 0xff) shl 8) or (b[at + 3].toLong() and 0xff)

    fun sealRequest(key: ByteArray, challenge: ByteArray, op: WatchOp, counter: Long, body: ByteArray = ByteArray(0)): ByteArray {
        require(counter in 1..0xffff_ffffL)
        val head = byteArrayOf(op.code.toByte()) + u32(counter) + body
        return head + hmac(key, TAG_REQUEST, challenge, head)
    }

    /** The request in [frame] if it is whole and its MAC holds under [key] and [challenge]; else null. */
    fun openRequest(key: ByteArray, challenge: ByteArray, frame: ByteArray): Request? {
        if (frame.size < 5 + MAC_LEN || frame.size > MAX_FRAME) return null
        val head = frame.copyOfRange(0, frame.size - MAC_LEN)
        val mac = frame.copyOfRange(frame.size - MAC_LEN, frame.size)
        if (!same(mac, hmac(key, TAG_REQUEST, challenge, head))) return null
        val op = WatchOp.of(head[0].toInt() and 0xff) ?: return null
        return Request(op, readU32(head, 1), head.copyOfRange(5, head.size))
    }

    class Response(val status: WatchStatus, val body: ByteArray)

    fun sealResponse(key: ByteArray, challenge: ByteArray, counter: Long, status: WatchStatus, body: ByteArray = ByteArray(0)): ByteArray {
        val head = byteArrayOf(status.code.toByte()) + body
        return head + hmac(key, TAG_RESPONSE, challenge, u32(counter), head)
    }

    fun openResponse(key: ByteArray, challenge: ByteArray, counter: Long, frame: ByteArray): Response? {
        if (frame.size < 1 + MAC_LEN || frame.size > MAX_FRAME) return null
        val head = frame.copyOfRange(0, frame.size - MAC_LEN)
        val mac = frame.copyOfRange(frame.size - MAC_LEN, frame.size)
        if (!same(mac, hmac(key, TAG_RESPONSE, challenge, u32(counter), head))) return null
        val status = WatchStatus.of(head[0].toInt() and 0xff) ?: return null
        return Response(status, head.copyOfRange(1, head.size))
    }

    /** One frame on a stream: its length (u16), then its bytes. */
    fun write(out: OutputStream, frame: ByteArray) {
        require(frame.size in 1..MAX_FRAME)
        val data = DataOutputStream(out)
        data.writeShort(frame.size)
        data.write(frame)
        data.flush()
    }

    /** The next frame, or null at the end of the stream; a size past [MAX_FRAME] is an [java.io.IOException]. */
    fun read(input: InputStream): ByteArray? {
        val data = DataInputStream(input)
        val size = try {
            data.readUnsignedShort()
        } catch (_: EOFException) {
            return null
        }
        if (size < 1 || size > MAX_FRAME) throw java.io.IOException("a frame of $size bytes")
        val frame = ByteArray(size)
        data.readFully(frame)
        return frame
    }
}

/** A body's fields, written in order; a string is its length (u8) and its UTF-8 bytes. */
class WatchOut {
    private val bytes = ByteArrayOutputStream()

    fun u8(n: Int) = apply { bytes.write(n) }

    fun u16(n: Int) = apply {
        bytes.write(n shr 8)
        bytes.write(n)
    }

    fun str(s: String) = apply {
        val raw = WatchText.fit(s).toByteArray(Charsets.UTF_8)
        bytes.write(raw.size)
        bytes.write(raw)
    }

    fun bytes(): ByteArray = bytes.toByteArray()
}

/** A body read back; anything short, or left over, is [Malformed]. */
class WatchIn(private val b: ByteArray) {
    class Malformed : Exception("a body that does not read")

    private var at = 0

    fun u8(): Int {
        if (at >= b.size) throw Malformed()
        return b[at++].toInt() and 0xff
    }

    fun u16(): Int = (u8() shl 8) or u8()

    fun str(): String {
        val n = u8()
        if (at + n > b.size) throw Malformed()
        val s = String(b, at, n, Charsets.UTF_8)
        at += n
        return s
    }

    /** Fails unless the whole body was read. */
    fun end() {
        if (at != b.size) throw Malformed()
    }
}
