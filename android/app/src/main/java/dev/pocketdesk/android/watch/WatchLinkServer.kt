package dev.pocketdesk.android.watch

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.watchlink.PairingWindow
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchLimits
import dev.pocketdesk.watchlink.WatchPairing
import dev.pocketdesk.watchlink.WatchSession
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

/**
 * The phone's end of the watch link (item 9): while "Watch controls" is on,
 * a Bluetooth service for the paired watch to call. Plain `android.bluetooth`
 * (a secure RFCOMM socket, so the watch must be bonded), with no Google
 * Play services and no Data Layer, which keeps the app F-Droid clean. It
 * lives in the notification listener's process, which the system keeps
 * bound, and is started and stopped by [sync] on the listener's tick.
 *
 * Only the paired watch's own Bluetooth address is let in (the pairing
 * window apart), each frame is checked ([WatchSession]), and nothing is
 * logged but that a thing happened: never an address, a key or a question.
 */
object WatchLinkServer {
    private const val TAG = "WatchLink"

    /** A connection with no frame for this long is closed: a tap makes a short one. */
    private const val IDLE_MS = 30_000L

    /** Two at a time: a stuck connection never keeps the watch out. */
    private const val MAX_CONNECTIONS = 2

    /** The window the pairing screen opens. */
    val pairing = PairingWindow({ System.currentTimeMillis() })

    private val limits = WatchLimits({ android.os.SystemClock.elapsedRealtime() })
    private val watchdog = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "watch-link-idle").apply { isDaemon = true } }
    private val connections = AtomicInteger(0)
    private val pairLock = Any()

    @Volatile
    private var server: BluetoothServerSocket? = null

    @Volatile
    private var running = false

    /** Whether the phone may use Bluetooth for this: the nearby-devices permission, asked when Watch controls is turned on. */
    fun hasPermission(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

    /** Start or stop the service to match the switch, the permission and Bluetooth itself. Cheap when nothing changed. */
    @Synchronized
    fun sync(ctx: Context) {
        val app = ctx.applicationContext
        val wanted = Prefs.watchControls(app) && hasPermission(app)
        if (!wanted) {
            if (running) stop()
            return
        }
        if (running) return
        start(app)
    }

    @SuppressLint("MissingPermission")
    private fun start(app: Context) {
        val adapter = app.getSystemService(BluetoothManager::class.java)?.adapter?.takeIf { it.isEnabled } ?: return
        val socket = try {
            adapter.listenUsingRfcommWithServiceRecord(WatchWire.SERVICE_NAME, WatchWire.SERVICE)
        } catch (e: IOException) {
            Log.w(TAG, "the watch service did not start: ${e.javaClass.simpleName}")
            return
        } catch (e: SecurityException) {
            Log.w(TAG, "the watch service did not start: no permission")
            return
        }
        server = socket
        running = true
        Log.i(TAG, "the watch service started")
        thread(name = "watch-link-accept") {
            try {
                while (running) {
                    val client = socket.accept()
                    if (connections.incrementAndGet() > MAX_CONNECTIONS) {
                        connections.decrementAndGet()
                        runCatching { client.close() }
                        continue
                    }
                    thread(name = "watch-link-conn") {
                        try {
                            handle(app, client)
                        } catch (e: Exception) {
                            Log.i(TAG, "a watch connection ended: ${e.javaClass.simpleName}")
                        } finally {
                            runCatching { client.close() }
                            connections.decrementAndGet()
                        }
                    }
                }
            } catch (_: IOException) {
                // Closed by stop(), or Bluetooth went off.
            } finally {
                synchronized(this@WatchLinkServer) {
                    if (server === socket) {
                        server = null
                        running = false
                    }
                }
                runCatching { socket.close() }
            }
        }
    }

    @Synchronized
    fun stop() {
        running = false
        pairing.close()
        runCatching { server?.close() }
        server = null
        Log.i(TAG, "the watch service stopped")
    }

    private fun handle(app: Context, client: BluetoothSocket) {
        val paired = WatchStore.paired(app)
        val address = runCatching { client.remoteDevice.address }.getOrNull() ?: return
        val known = paired != null && paired.address == address
        // Only the paired watch, or whichever bonded device the owner is pairing now.
        if (!known && !pairing.isOpen) return
        val challenge = WatchPairing.newChallenge()
        val out = client.outputStream
        val input = client.inputStream
        val idle = IdleClose(client)
        try {
            idle.touch(if (known) IDLE_MS else PairingWindow.LIFE_MS + 10_000)
            WatchWire.write(out, WatchWire.hello(challenge, paired = known))
            val first = WatchWire.read(input) ?: return
            if (first[0].toInt() == WatchWire.PAIR_TAG) {
                if (pairing.isOpen) pair(app, client, challenge, first, address)
                return
            }
            val held = paired?.takeIf { known } ?: return
            val session = WatchSession(
                key = held.key,
                challenge = challenge,
                actions = PhoneWatchActions(app),
                on = { if (Prefs.watchControls(app)) Prefs.watchButtons(app) else emptySet<WatchButton>() },
                limits = limits,
            )
            var frame: ByteArray? = first
            while (frame != null) {
                idle.touch(IDLE_MS)
                session.serve(frame)?.let { WatchWire.write(out, it) }
                if (session.dropped) {
                    Log.i(TAG, "a watch connection was ended: frames that did not verify")
                    return
                }
                frame = WatchWire.read(input)
            }
        } finally {
            idle.cancel()
        }
    }

    /** The watch's pair frame: wait for the code the owner types on the phone, judge it, hand the key over masked. */
    private fun pair(app: Context, client: BluetoothSocket, challenge: ByteArray, first: ByteArray, address: String) {
        val frame = WatchPairing.readPairFrame(first)
        if (frame == null) {
            WatchWire.write(client.outputStream, WatchPairing.refused())
            return
        }
        // One pairing at a time.
        synchronized(pairLock) {
            while (pairing.isOpen) {
                val code = pairing.typed()
                if (code == null) {
                    Thread.sleep(200)
                    continue
                }
                val right = WatchWire.same(WatchPairing.proof(code, challenge, frame.name), frame.proof)
                pairing.judged(right)
                if (!right) {
                    Log.i(TAG, "a pairing code was wrong")
                    WatchWire.write(client.outputStream, WatchPairing.refused())
                    return
                }
                val key = WatchPairing.newKey()
                WatchStore.save(app, key, address, frame.name)
                WatchWire.write(client.outputStream, WatchPairing.accepted(WatchPairing.mask(code, challenge, key)))
                Log.i(TAG, "a watch was paired")
                return
            }
            WatchWire.write(client.outputStream, WatchPairing.refused())
        }
    }

    /** Closes a socket that goes quiet: a read on a Bluetooth stream has no timeout of its own. */
    private class IdleClose(private val socket: BluetoothSocket) {
        private var pending: ScheduledFuture<*>? = null

        @Synchronized
        fun touch(ms: Long) {
            pending?.cancel(false)
            pending = watchdog.schedule({ runCatching { socket.close() } }, ms, TimeUnit.MILLISECONDS)
        }

        @Synchronized
        fun cancel() {
            pending?.cancel(false)
        }
    }
}
