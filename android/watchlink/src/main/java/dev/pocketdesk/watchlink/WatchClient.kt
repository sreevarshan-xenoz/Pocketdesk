package dev.pocketdesk.watchlink

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * The watch's end of one connection to the phone, over any pair of streams
 * (the watch app gives it a Bluetooth socket's; the tests, the phone's own
 * session). One connection, a few calls, closed: a tap makes one.
 *
 * Failures are [IOException]s: the phone did not speak, spoke wrongly, or an
 * answer did not carry the MAC only the phone with this watch's key can make.
 */
class WatchClient(
    private val input: InputStream,
    private val output: OutputStream,
    private val key: ByteArray?,
) {
    private var challenge: ByteArray? = null
    private var counter = 0L

    /** Read the phone's hello; whether the phone holds a key for this watch. */
    fun open(): Boolean {
        val frame = WatchWire.read(input) ?: throw IOException("the phone did not speak")
        val hello = WatchWire.readHello(frame) ?: throw IOException("that is not the phone's service")
        challenge = hello.challenge
        return hello.paired
    }

    /** Ask the phone for [op] with [body]; its answer if it verifies. */
    fun call(op: WatchOp, body: ByteArray = ByteArray(0)): WatchWire.Response {
        val key = key ?: throw IOException("no key")
        val challenge = challenge ?: throw IOException("not opened")
        val at = ++counter
        WatchWire.write(output, WatchWire.sealRequest(key, challenge, op, at, body))
        val frame = WatchWire.read(input) ?: throw IOException("the phone hung up")
        return WatchWire.openResponse(key, challenge, at, frame) ?: throw IOException("an answer that does not verify")
    }

    /**
     * Pair: prove the six digits [code] read off this watch, as [name]. The
     * key the phone hands over when the owner typed the same digits; an
     * [IOException] when it refused or never answered.
     */
    fun pair(code: String, name: String): ByteArray {
        val challenge = challenge ?: throw IOException("not opened")
        WatchWire.write(output, WatchPairing.pairFrame(code, challenge, name))
        val frame = WatchWire.read(input) ?: throw IOException("the phone hung up")
        return WatchPairing.readAnswer(code, challenge, frame) ?: throw IOException("the phone refused")
    }
}
