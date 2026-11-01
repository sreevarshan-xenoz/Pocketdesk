package dev.pocketdesk.wear

import android.Manifest
import android.annotation.SuppressLint
import android.app.KeyguardManager
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchClient
import dev.pocketdesk.watchlink.WatchOp
import dev.pocketdesk.watchlink.WatchStatus
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException
import java.util.Timer
import java.util.TimerTask

/** The watch has no key for a phone, or the phone does not hold one for this watch. */
class NotPairedException : IOException("not paired")

/** The watch may not use Bluetooth yet (the permission), or it is off. */
class NoBluetoothException : IOException("no bluetooth")

/**
 * The watch's connection to its phone: a secure RFCOMM socket to the
 * Pocketdesk service on the phone this watch is paired with, over the
 * Bluetooth bond the two already have. Plain `android.bluetooth`, with no
 * Google Play services. A phone out of range or a Bluetooth that is off
 * ends in an [IOException] within [TIMEOUT_MS]: the watch has no standalone
 * mode (everything goes through the phone's own link to the computer), and
 * nothing is queued to go later.
 */
object PhoneLink {
    /** The whole connection, from connecting to the last answer: a tap gives up after this. */
    const val TIMEOUT_MS = 12_000L

    fun hasPermission(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            ctx.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

    /** Run [block] on one connection to the paired phone; closed when it ends or when [timeoutMs] runs out. */
    @SuppressLint("MissingPermission")
    fun <T> session(ctx: Context, timeoutMs: Long = TIMEOUT_MS, block: (WatchClient) -> T): Result<T> {
        val app = ctx.applicationContext
        val paired = PhoneStore.paired(app) ?: return Result.failure(NotPairedException())
        if (!hasPermission(app)) return Result.failure(NoBluetoothException())
        val adapter = app.getSystemService(BluetoothManager::class.java)?.adapter?.takeIf { it.isEnabled }
            ?: return Result.failure(NoBluetoothException())
        return try {
            val socket = adapter.getRemoteDevice(paired.address).createRfcommSocketToServiceRecord(WatchWire.SERVICE)
            // A read on a Bluetooth stream has no timeout of its own: the socket is closed under it.
            val timer = Timer("phone-link-timeout", true)
            timer.schedule(object : TimerTask() {
                override fun run() {
                    runCatching { socket.close() }
                }
            }, timeoutMs)
            try {
                adapter.cancelDiscovery()
                socket.connect()
                val client = WatchClient(socket.inputStream, socket.outputStream, paired.key)
                if (!client.open()) throw NotPairedException()
                Result.success(block(client))
            } finally {
                timer.cancel()
                runCatching { socket.close() }
            }
        } catch (e: IOException) {
            Result.failure(e)
        } catch (e: SecurityException) {
            Result.failure(NoBluetoothException())
        } catch (e: IllegalArgumentException) {
            // An address that is not one (a store gone bad).
            Result.failure(NotPairedException())
        }
    }
}

/** What the phone said it has and is waiting on. */
class Look(val computer: String, val buttons: Set<WatchButton>, val asks: List<WatchBodies.Ask>)

/** What one tap came to, and what the phone said after it; [deck] is its deck's buttons when asked for. */
class Run(val line: TilePlan.Line, val look: Result<Look>, val deck: List<WatchBodies.Deck> = emptyList())

/** The requests the tile and the list make, in one place. */
object Phone {
    private const val LIST_TIMEOUT_MS = 20_000L

    /** The phone's status and, when it has buttons on and questions waiting, the questions. */
    fun look(client: WatchClient): Look {
        val answer = client.call(WatchOp.Status)
        if (answer.status != WatchStatus.Ok) throw IOException("status refused")
        val status = WatchBodies.readStatus(answer.body) ?: throw IOException("a status that does not read")
        val asks = if (WatchButton.Asks in status.buttons && status.asks > 0) {
            val list = client.call(WatchOp.AskList)
            if (list.status == WatchStatus.Ok) WatchBodies.readAsks(list.body).orEmpty() else emptyList()
        } else {
            emptyList()
        }
        return Look(status.computer, status.buttons, asks)
    }

    fun send(client: WatchClient, act: TilePlan.Act): WatchWire.Response = when (act) {
        TilePlan.Act.Wake -> client.call(WatchOp.Wake)
        TilePlan.Act.Ring -> client.call(WatchOp.Ring)
        is TilePlan.Act.Music -> client.call(WatchOp.Music, WatchBodies.music(act.command))
        is TilePlan.Act.Deny -> client.call(WatchOp.AskDeny, WatchBodies.askDeny(act.id))
        is TilePlan.Act.Deck -> client.call(WatchOp.DeckPress, WatchBodies.deckPress(act.index, act.label))
    }

    /** The watch's own lock in front of [op]: [WatchLockRule] with this watch's keyguard. */
    fun lockVerdict(ctx: Context, op: WatchOp): WatchLockRule.Verdict {
        val keyguard = ctx.getSystemService(KeyguardManager::class.java)
        return WatchLockRule.check(op, keyguard?.isDeviceSecure == true, keyguard?.isDeviceLocked != false)
    }

    /**
     * One connection: [act] if there is one (behind the watch's lock where
     * [WatchLockRule] says so), then a look at what the phone has on.
     */
    fun run(ctx: Context, act: TilePlan.Act?, withDeck: Boolean = false): Run {
        var line: TilePlan.Line = TilePlan.Line.Nothing
        var deck: List<WatchBodies.Deck> = emptyList()
        // A deck press waits on the computer: the list's connection gets longer than a tile's.
        val looked = PhoneLink.session(ctx, if (withDeck || act is TilePlan.Act.Deck) LIST_TIMEOUT_MS else PhoneLink.TIMEOUT_MS) { client ->
            if (act != null) {
                line = when (lockVerdict(ctx, act.op)) {
                    WatchLockRule.Verdict.NoLockSet -> TilePlan.Line.NeedsLockSet
                    WatchLockRule.Verdict.Locked -> TilePlan.Line.NeedsUnlock
                    WatchLockRule.Verdict.Allowed -> TilePlan.line(act, runCatching { send(client, act) })
                }
            }
            look(client).also { seen ->
                if (withDeck && WatchButton.Deck in seen.buttons) {
                    val listed = client.call(WatchOp.DeckList)
                    if (listed.status == WatchStatus.Ok) deck = WatchBodies.readDeck(listed.body).orEmpty()
                }
            }
        }
        if (act != null && line == TilePlan.Line.Nothing) line = failed(looked.exceptionOrNull())
        return Run(line, looked, deck)
    }

    /** The line for a connection that did not come to be. */
    fun failed(e: Throwable?): TilePlan.Line = when (e) {
        is NotPairedException -> TilePlan.Line.NotPaired
        is NoBluetoothException -> TilePlan.Line.NoBluetooth
        else -> TilePlan.Line.Unreachable
    }
}
