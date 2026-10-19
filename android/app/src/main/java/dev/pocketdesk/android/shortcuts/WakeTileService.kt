package dev.pocketdesk.android.shortcuts

import android.os.Build
import android.os.Handler
import android.os.Looper
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.plainErrorText
import dev.pocketdesk.android.wake.WakePlan
import dev.pocketdesk.android.wake.Waker
import java.lang.ref.WeakReference
import kotlin.concurrent.thread

/**
 * Quick Settings tile (G7): wakes the last computer as its Wake settings
 * say (N6), from the shade with no app open. Its subtitle says whether a
 * wake can go from here, and for a minute after a tap how it went. A
 * locked phone is unlocked first.
 */
class WakeTileService : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        open = WeakReference(this)
        draw()
    }

    override fun onStopListening() {
        if (open?.get() === this) open = null
        super.onStopListening()
    }

    private fun draw() {
        val tile = qsTile ?: return
        val host = HostStore.lastHost(this)
        // Reading the plan touches only saved settings and the network list, never the computer.
        val plan = host?.let { runCatching { Waker.planFor(this, it.address) }.getOrNull() }
        val sent = last?.takeIf { it.first == host?.address }?.second
        // A plan that cannot be read now is said as not ready yet.
        val said = WakeTilePlan.said(if (host == null) null else plan ?: WakePlan.Plan.NoMac, sent?.send, System.currentTimeMillis())
        val subtitle = when (said) {
            WakeTilePlan.Said.NoComputer -> getString(R.string.tile_no_computer)
            WakeTilePlan.Said.Ready -> host?.let { HostStore.shownName(this, it.address) }
            WakeTilePlan.Said.NotYet -> getString(R.string.wake_tile_not_yet)
            WakeTilePlan.Said.Off -> getString(R.string.wake_tile_off)
            WakeTilePlan.Said.Sent -> getString(R.string.wake_tile_sent)
            WakeTilePlan.Said.Failed -> sent?.why ?: getString(R.string.wake_tile_failed)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.label = getString(R.string.wake_tile_label)
            tile.subtitle = subtitle
        } else {
            // No subtitle before Android 10.
            tile.label = if (said == WakeTilePlan.Said.Ready) getString(R.string.wake_tile_label) else subtitle
        }
        tile.state = if (host == null) Tile.STATE_UNAVAILABLE else Tile.STATE_INACTIVE
        tile.updateTile()
    }

    override fun onClick() {
        super.onClick()
        val host = HostStore.lastHost(this) ?: return
        // Behind the phone's lock, as the other tiles' actions are.
        if (isLocked) unlockAndRun { wake(host.address) } else wake(host.address)
    }

    private fun wake(address: String) {
        val app = applicationContext
        thread(name = "wake-tile") {
            val result = try {
                // A tap may start the process: the bridge first (G3's lesson).
                RustCore.start(app)
                Waker.wake(app, address)
            } catch (e: Throwable) {
                Result.failure(e)
            }
            result.exceptionOrNull()?.let { Log.w(TAG, "wake failed", it) }
            val why = result.exceptionOrNull()?.let { app.plainErrorText(it.message) }
            last = address to Sent(WakeTilePlan.Send(result.isSuccess, System.currentTimeMillis()), why)
            Handler(Looper.getMainLooper()).post { open?.get()?.draw() }
        }
    }

    /** The last tap's outcome, and why it failed. */
    private data class Sent(val send: WakeTilePlan.Send, val why: String?)

    companion object {
        private const val TAG = "WakeTile"

        /**
         * The tile while the panel shows it: only an active tile may ask to
         * be drawn again, so the open one is drawn directly after a tap.
         */
        @Volatile
        private var open: WeakReference<WakeTileService>? = null

        /** The last tap: the computer and how it went. */
        @Volatile
        private var last: Pair<String, Sent>? = null
    }
}
