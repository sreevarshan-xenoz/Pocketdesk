package dev.pocketdesk.android.bluetooth

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.system.Os
import android.system.OsConstants
import android.util.Log
import androidx.core.content.ContextCompat
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.thread

/**
 * The phone's Bluetooth road to its computer (protocol.md 1.4a): when
 * neither home nor the internet reaches it, or the phone has no usable
 * network, a secure RFCOMM connection to the bonded device the computer
 * named, pumped through a socket pair into the bridge, which runs the
 * control link on it. While that link lives, each transfer link the
 * computer asks for gets a connection of its own to the transfer service.
 *
 * Nothing scans or discovers: only a device the computer told the phone
 * of, and bonded already, is dialed ([BluetoothRoadRule.shouldDial]).
 * Nothing is kept up while another road works. Never an address in the
 * log.
 */
object BluetoothRoad {
    private const val TAG = "BluetoothRoad"

    /** The longest a dial to the computer may take. */
    private const val CONNECT_MS = 12_000L

    /** How often, while the road lives, a link request is looked for. */
    private const val POLL_MS = 500L

    /** How often, while the road lives with a local network up, the phone looks whether home answers again. */
    private const val HOME_CHECK_MS = 15_000L

    /** Once a network is back, a moment for its address and routes before home is tried. */
    private const val SETTLE_MS = 3_000L

    private const val BUFFER = 16 * 1024

    /** Failed dials in a row and the rest after them, by the computer's address. */
    private class Rest(var fails: Int = 0, var until: Long = 0L)

    private val rests = HashMap<String, Rest>()

    /** The computers (by [dial]'s address) the log said are not bonded, said once until they are. */
    private val toldUnbonded = HashSet<String>()

    /** One dial at a time: the many callers of a connect do not each open one. */
    private val dialing = ReentrantLock()

    /** The control link's road, while it lives. */
    @Volatile
    private var live: Road? = null

    /**
     * The road to the computer at [address] (its control [port]): the
     * RFCOMM [socket] and the [pipe] into the bridge, and the transfer
     * link open now, if any.
     */
    private class Road(
        val address: String,
        val port: Int,
        val device: BluetoothDevice,
        val link: UUID,
        val pipe: Pipe,
    ) {
        @Volatile
        var transfer: Pipe? = null
    }

    /** Whether the phone may use Bluetooth: the nearby-devices permission (Android 12 on). */
    fun hasPermission(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

    private fun adapter(ctx: Context): BluetoothAdapter? =
        ctx.getSystemService(BluetoothManager::class.java)?.adapter

    /** Whether the road may be used at all: the switch, the permission and the radio. */
    private fun usable(ctx: Context): Boolean =
        Prefs.bluetoothFallback(ctx) && hasPermission(ctx) && radioOn(ctx)

    private fun radioOn(ctx: Context): Boolean =
        runCatching { adapter(ctx)?.isEnabled == true }.getOrDefault(false)

    /** Whether a control link to the computer at [address] runs on this road now: its transfers go this way. */
    fun carries(address: String): Boolean =
        live?.let { it.address == address && it.pipe.alive } == true

    /** Whether a control link to [address]:[port] runs on this road now. */
    fun isLive(address: String, port: Int): Boolean =
        live?.let { it.address == address && it.port == port && it.pipe.alive } == true

    /**
     * Dial the computer at [address] by Bluetooth when [BluetoothRoadRule]
     * says to: [otherRoadsFailed] after home and the internet failed, or
     * first when the phone has no usable network ([hasUsableNetwork]).
     * The bridge's envelope when it dialed; null when it did not (the rule
     * said no, or another dial runs now).
     */
    @SuppressLint("MissingPermission") // hasPermission is checked first, and a SecurityException caught.
    fun dial(ctx: Context, address: String, port: Int, hasUsableNetwork: Boolean, otherRoadsFailed: Boolean): Result<String>? {
        // A dial waits up to 12 s: never on the main thread.
        if (Looper.myLooper() == Looper.getMainLooper()) return null
        val app = ctx.applicationContext
        val info = BluetoothRoadRule.info(HostStore.bluetoothInfo(app, address)) ?: return null
        val permitted = hasPermission(app)
        val adapter = adapter(app) ?: return null
        val radioOn = permitted && runCatching { adapter.isEnabled }.getOrDefault(false)
        val bonded = if (permitted && radioOn) bondedAddresses(adapter) else emptySet()
        val now = SystemClock.elapsedRealtime()
        val restUntil = synchronized(rests) { rests[address]?.until ?: 0L }
        val go = BluetoothRoadRule.shouldDial(
            enabled = Prefs.bluetoothFallback(app),
            permitted = permitted,
            radioOn = radioOn,
            knownAddress = info.address,
            bonded = bonded,
            hasUsableNetwork = hasUsableNetwork,
            otherRoadsFailed = otherRoadsFailed,
            now = now,
            restUntil = restUntil,
        )
        noteBond(
            address,
            BluetoothRoadRule.unbonded(
                enabled = Prefs.bluetoothFallback(app),
                permitted = permitted,
                radioOn = radioOn,
                knownAddress = info.address,
                bonded = bonded,
            ),
        )
        if (!go || !dialing.tryLock()) return null
        try {
            if (isLive(address, port)) return Result.success("Connected")
            // Another computer's road gives way to this one.
            stop()
            val result = runCatching { open(app, adapter, info, address, port) }
                .getOrElse { Result.failure(it) }
            if (isLive(address, port)) {
                synchronized(rests) { rests.remove(address) }
            } else {
                failed(address)
            }
            return result
        } finally {
            dialing.unlock()
        }
    }

    @SuppressLint("MissingPermission")
    private fun bondedAddresses(adapter: BluetoothAdapter): Set<String> =
        try {
            adapter.bondedDevices.orEmpty().mapNotNull { it.address?.uppercase() }.toSet()
        } catch (_: SecurityException) {
            emptySet()
        }

    /** Say once that the computer at [address] is not dialed for want of a bond ([BluetoothRoadRule.unbonded]); again only after it was. */
    private fun noteBond(address: String, unbonded: Boolean) {
        val first = synchronized(toldUnbonded) {
            if (unbonded) {
                toldUnbonded.add(address)
            } else {
                toldUnbonded.remove(address)
                false
            }
        }
        if (first) {
            Log.i(TAG, "not dialing the computer over Bluetooth: this phone is not bonded with it (pair them once in Android's Bluetooth settings)")
        }
    }

    private fun failed(address: String) {
        synchronized(rests) {
            val rest = rests.getOrPut(address) { Rest() }
            rest.fails++
            rest.until = SystemClock.elapsedRealtime() + BluetoothRoadRule.nextRest(rest.fails)
        }
    }

    /**
     * The phone's network changed (one came back, or the first since a
     * start with none): the rest after failed dials is let go, and a live
     * road is checked once for leaving ([moveOffIfBack]).
     */
    fun networkBack(ctx: Context) {
        synchronized(rests) { rests.clear() }
        val road = live ?: return
        val app = ctx.applicationContext
        thread(name = "bt-road-home", isDaemon = true) {
            runCatching {
                Thread.sleep(SETTLE_MS)
                moveOffIfBack(app, road, networkChanged = true)
            }.onFailure { Log.w(TAG, "the move off Bluetooth failed: ${it.javaClass.simpleName}") }
        }
    }

    /** End the road: its sockets close, the pumps end, and the bridge sees the stream end. */
    fun stop() {
        val road = live ?: return
        live = null
        road.transfer?.close()
        road.pipe.close()
        Log.i(TAG, "the Bluetooth road ended")
    }

    /** Open the road: dial, pump, hand the bridge its end. Under [dialing]. */
    @SuppressLint("MissingPermission")
    private fun open(app: Context, adapter: BluetoothAdapter, info: BluetoothRoadRule.Info, address: String, port: Int): Result<String> {
        val device = try {
            adapter.getRemoteDevice(info.address)
        } catch (e: IllegalArgumentException) {
            return Result.failure(e)
        }
        val socket = connect(app, device, info.service, CONNECT_MS)
            ?: return Result.failure(IOException("The computer did not answer by Bluetooth"))
        val pipe = try {
            Pipe.of(socket)
        } catch (e: IOException) {
            runCatching { socket.close() }
            return Result.failure(e)
        }
        val road = Road(address, port, device, info.link, pipe)
        pipe.onEnd = {
            if (live === road) live = null
            road.transfer?.close()
        }
        // Given up before the pumps run, so no close can race the detach.
        val theirs = pipe.detachTheirs()
        pipe.start("bt-road")
        val result = RustCore.connectOverBluetooth(address, port, theirs)
        val up = RustCore.controlPeer() == (address to port) &&
            RustCore.controlRoad() == BluetoothRoadRule.ROAD_BLUETOOTH
        if (!up) {
            // Refused, or another road came up meanwhile: this one closes.
            pipe.close()
            return result
        }
        // Live only once the bridge has the link up (after its TLS), so
        // nothing counts on the road before it carries anything.
        live = road
        if (!pipe.alive) {
            live = null
            return result
        }
        Log.i(TAG, "the control link runs over Bluetooth")
        // The first time only, the owner hears of it.
        runCatching { BluetoothNotice.maybeTell(app, address) }
        thread(name = "bt-road-links", isDaemon = true) { tend(app, road) }
        return result
    }

    /**
     * Dial [device]'s service [service] within [ms]: `connect()` blocks on
     * a thread of its own, and is closed when it takes longer. The socket,
     * or null.
     */
    @SuppressLint("MissingPermission")
    private fun connect(app: Context, device: BluetoothDevice, service: UUID, ms: Long): BluetoothSocket? {
        if (!usable(app)) return null
        val socket = try {
            device.createRfcommSocketToServiceRecord(service)
        } catch (_: IOException) {
            return null
        } catch (_: SecurityException) {
            return null
        }
        val done = CountDownLatch(1)
        var error: Throwable? = null
        thread(name = "bt-road-connect", isDaemon = true) {
            try {
                socket.connect()
            } catch (e: Throwable) {
                error = e
            } finally {
                done.countDown()
            }
        }
        if (!done.await(ms, TimeUnit.MILLISECONDS) || error != null) {
            runCatching { socket.close() }
            return null
        }
        return socket
    }

    /**
     * While [road] lives: answer the computer's link requests one at a
     * time, end it when the switch, the permission or the radio goes, and
     * move home once home answers again.
     */
    private fun tend(app: Context, road: Road) {
        try {
            tendLoop(app, road)
        } catch (e: Exception) {
            // A stray failure ends the road, never the app.
            Log.w(TAG, "the Bluetooth road failed: ${e.javaClass.simpleName}")
            if (live === road) stop() else road.pipe.close()
        }
    }

    private fun tendLoop(app: Context, road: Road) {
        var lastHomeCheck = SystemClock.elapsedRealtime()
        while (live === road && road.pipe.alive) {
            if (!usable(app)) {
                Log.i(TAG, "Bluetooth is off or not allowed: the road ends")
                stop()
                return
            }
            if (BluetoothRoadRule.takeLinkRequest(road.transfer?.alive == true)) {
                RustCore.takeBluetoothLinkRequest()?.let { answer(app, road, it) }
            }
            val now = SystemClock.elapsedRealtime()
            if (now - lastHomeCheck >= HOME_CHECK_MS) {
                lastHomeCheck = now
                moveOffIfBack(app, road, networkChanged = false)
                // The probe took a few seconds: a link request that came
                // meanwhile is taken at once, not after another poll.
                continue
            }
            Thread.sleep(POLL_MS)
        }
    }

    /**
     * The computer asked for transfer link [id]: a new connection to its
     * transfer service, tried again for a few seconds while the last one's
     * channel is still closing, handed to the bridge (which blocks while it
     * says the hello and runs the TLS).
     */
    private fun answer(app: Context, road: Road, id: Long) {
        val started = SystemClock.elapsedRealtime()
        var socket: BluetoothSocket? = null
        while (socket == null && live === road) {
            val left = started + BluetoothRoadRule.LINK_TRY_MS - SystemClock.elapsedRealtime()
            if (left <= 0) break
            socket = connect(app, road.device, road.link, left)
            if (socket == null) {
                if (!BluetoothRoadRule.tryLinkAgain(started, SystemClock.elapsedRealtime())) break
                Thread.sleep(BluetoothRoadRule.LINK_STEP_MS)
            }
        }
        if (socket == null) {
            Log.i(TAG, "a transfer link did not open")
            return
        }
        val pipe = try {
            Pipe.of(socket)
        } catch (_: IOException) {
            runCatching { socket.close() }
            return
        }
        road.transfer = pipe
        val theirs = pipe.detachTheirs()
        pipe.start("bt-road-link")
        if (!RustCore.offerBluetoothLink(id, theirs)) {
            pipe.close()
            Log.i(TAG, "a transfer link was not taken")
        }
    }

    /**
     * Whether to leave Bluetooth for the computer's usual roads
     * ([BluetoothRoadRule.leaveBluetooth]): on the 15 s tick only home is
     * checked, and only with a local network up (Wi-Fi, Ethernet or the
     * phone's hotspot), so mobile data alone costs no probe
     * every 15 s; on a network change ([networkChanged]) home is checked
     * once, and a computer with an internet road is dialed over it too. The
     * Bluetooth link is closed first (the bridge would otherwise take it as
     * the live one), then the computer dialed as usual: home, the internet,
     * then Bluetooth again if neither answers.
     */
    private fun moveOffIfBack(app: Context, road: Road, networkChanged: Boolean) {
        synchronized(road) { moveOffIfBackLocked(app, road, networkChanged) }
    }

    private fun moveOffIfBackLocked(app: Context, road: Road, networkChanged: Boolean) {
        if (live !== road || !road.pipe.alive) return
        // Another road's link replaced this one: its pipe ends by itself.
        if (RustCore.controlRoad() != BluetoothRoadRule.ROAD_BLUETOOTH) return
        val probe = BluetoothRoadRule.probeHome(
            networkChanged = networkChanged,
            usableNetwork = dev.pocketdesk.android.service.NetworkWait.usable(app, road.address),
            localNetwork = dev.pocketdesk.android.service.NetworkWait.local(app),
        )
        if (!probe) return
        val homeUp = RustCore.checkIdentity(road.address, road.port).getOrNull() == "matches"
        val internetRoad = HostStore.wanIdentity(app, road.address) != null
        if (!BluetoothRoadRule.leaveBluetooth(homeUp, networkChanged, internetRoad) || live !== road) return
        Log.i(TAG, if (homeUp) "home answers again: off Bluetooth" else "a network is back: trying the internet road")
        RustCore.closeControlLinkTo(road.address)
        stop()
        RustCore.connectToPeer(road.address, road.port, HostStore.wanIdentity(app, road.address))
    }

    /**
     * An RFCOMM [socket] pumped both ways to one end of a socket pair; the
     * other end goes to the bridge. When either side ends, both close.
     */
    private class Pipe private constructor(
        private val socket: BluetoothSocket,
        private val mine: ParcelFileDescriptor,
        private var theirs: ParcelFileDescriptor?,
    ) {
        @Volatile
        var alive = true
            private set

        @Volatile
        var onEnd: (() -> Unit)? = null

        /** The pumps still running: the last one closes [mine], so its descriptor is never reused under a read. */
        private val pumps = java.util.concurrent.atomic.AtomicInteger(0)

        fun start(name: String) {
            synchronized(this) {
                if (!alive) return
                pumps.set(2)
            }
            thread(name = "$name-in", isDaemon = true) {
                pump({ socket.inputStream.read(it) }, FileOutputStream(mine.fileDescriptor))
            }
            thread(name = "$name-out", isDaemon = true) {
                val from = FileInputStream(mine.fileDescriptor)
                pump({ from.read(it) }, socket.outputStream)
            }
        }

        /** The bridge's end, given up: the bridge owns and closes it. -1 once closed. */
        @Synchronized
        fun detachTheirs(): Int {
            if (!alive) return -1
            val fd = theirs?.detachFd() ?: -1
            theirs = null
            return fd
        }

        private fun pump(read: (ByteArray) -> Int, to: java.io.OutputStream) {
            val buffer = ByteArray(BUFFER)
            try {
                while (alive) {
                    val n = read(buffer)
                    if (n < 0) break
                    if (n == 0) continue
                    to.write(buffer, 0, n)
                    to.flush()
                }
            } catch (_: Exception) {
                // The radio went, or the bridge closed its end.
            } finally {
                close()
                if (pumps.decrementAndGet() == 0) runCatching { mine.close() }
            }
        }

        /** Close both sides: a blocked read on either wakes and ends. */
        fun close() {
            synchronized(this) {
                if (!alive) return
                alive = false
                runCatching { socket.close() }
                runCatching { Os.shutdown(mine.fileDescriptor, OsConstants.SHUT_RDWR) }
                // Not started: no pump will close it.
                if (pumps.get() == 0) runCatching { mine.close() }
                runCatching { theirs?.close() }
                theirs = null
            }
            onEnd?.invoke()
        }

        companion object {
            fun of(socket: BluetoothSocket): Pipe {
                val (mine, theirs) = ParcelFileDescriptor.createSocketPair()
                return Pipe(socket, mine, theirs)
            }
        }
    }
}
