package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
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
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * Quick Settings tile (G7): wakes the last computer as its Wake settings
 * say (N6), from the shade with no app open. Its subtitle says whether a
 * wake can go from here, and after a tap whether the computer answered.
 * The wake itself runs under a broadcast ([WakeTileReceiver]), so closing
 * the shade does not cut it off. A locked phone is unlocked first.
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

    /**
     * Draw the tile. The plan reads the network list, so it is read on a
     * worker and drawn when ready; the tile shows the last look meanwhile.
     */
    private fun draw() {
        val app = applicationContext
        thread(name = "wake-tile-plan") {
            val host = HostStore.lastHost(app)
            val plan = host?.let { runCatching { Waker.planFor(app, it.address) }.getOrNull() ?: WakePlan.Plan.NoMac }
            main.post { drawWith(host, plan) }
        }
    }

    private fun drawWith(host: HostStore.Host?, plan: WakePlan.Plan?) {
        val tile = qsTile ?: return
        val tap = last?.takeIf { it.address == host?.address }
        val said = WakeTilePlan.said(plan, tap?.send, System.currentTimeMillis())
        val subtitle = when (said) {
            WakeTilePlan.Said.NoComputer -> getString(R.string.tile_no_computer)
            WakeTilePlan.Said.Ready -> host?.let { HostStore.shownName(this, it.address) }
            WakeTilePlan.Said.NotYet -> getString(R.string.wake_tile_not_yet)
            WakeTilePlan.Said.Off -> getString(R.string.wake_tile_off)
            WakeTilePlan.Said.Sending -> getString(R.string.wake_tile_sending)
            WakeTilePlan.Said.Waiting -> getString(R.string.wake_tile_waiting)
            WakeTilePlan.Said.Awake -> getString(R.string.wake_tile_awake)
            WakeTilePlan.Said.NoAnswer -> getString(R.string.wake_tile_no_answer)
            WakeTilePlan.Said.Failed -> WakeTilePlan.shortWhy(tap?.why) ?: getString(R.string.wake_tile_failed)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.label = getString(R.string.wake_tile_label)
            tile.subtitle = subtitle
        } else {
            // No subtitle before Android 10.
            tile.label = if (said == WakeTilePlan.Said.Ready) getString(R.string.wake_tile_label) else subtitle
        }
        // Inactive with no computer too: a tap then opens the app to add one (G7 review).
        tile.state = Tile.STATE_INACTIVE
        tile.updateTile()
        // An outcome that is said for a while is taken down when its time is up (G7 review).
        tap?.send?.takeIf { !it.step.running }?.let { send ->
            val left = send.atMs + WakeTilePlan.SAID_MS - System.currentTimeMillis()
            if (left > 0) main.postDelayed({ open?.get()?.draw() }, left + 500)
        }
    }

    override fun onClick() {
        super.onClick()
        val host = HostStore.lastHost(this)
        // Behind the phone's lock, as the other tiles' actions are.
        val go = if (host == null) ({ openApp() }) else ({ start(host.address) })
        if (isLocked) unlockAndRun(go) else go()
    }

    private fun start(address: String) {
        // A tap waits for the one under way (G7 review).
        if (!WakeTilePlan.mayStart(last?.takeIf { it.address == address }?.send)) return
        step(address, WakeTilePlan.Step.Sending)
        sendBroadcast(Intent(this, WakeTileReceiver::class.java).putExtra(WakeTileReceiver.EXTRA_ADDRESS, address))
    }

    private fun openApp() {
        val intent = Shortcuts.connectLast(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
            )
        } else {
            @Suppress("DEPRECATION")
            @android.annotation.SuppressLint("StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }

    /** The last tap: its computer, where it is, and why it failed. */
    internal data class Tap(val address: String, val send: WakeTilePlan.Send, val why: String?)

    companion object {
        private val main = Handler(Looper.getMainLooper())

        /**
         * The tile while the panel shows it: only an active tile may ask to
         * be drawn again, so the open one is drawn directly as a tap goes on.
         */
        @Volatile
        private var open: WeakReference<WakeTileService>? = null

        @Volatile
        private var last: Tap? = null

        /** A tap for [address] reached [step]; the open tile shows it. */
        internal fun step(address: String, step: WakeTilePlan.Step, why: String? = null) {
            last = Tap(address, WakeTilePlan.Send(step, System.currentTimeMillis()), why)
            main.post { open?.get()?.draw() }
        }
    }
}

/**
 * The wake a tile tap asked for (G7), under this broadcast's goAsync so the
 * shade closing does not cut it off, bounded by the broadcast's time; then
 * the computer's control port is watched for a minute, as the Home card
 * does, to say whether it woke. Not exported.
 */
class WakeTileReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val address = intent.getStringExtra(EXTRA_ADDRESS) ?: return
        val app = context.applicationContext
        val host = HostStore.hosts(app).firstOrNull { it.address == address } ?: return
        val pending = goAsync()
        thread(name = "wake-tile") {
            var woke = false
            try {
                val sender = thread(name = "wake-tile-send") {
                    val result = try {
                        // A tap may start the process: the bridge first (G3's lesson).
                        RustCore.start(app)
                        Waker.wake(app, address)
                    } catch (e: Throwable) {
                        Result.failure(e)
                    }
                    result.exceptionOrNull()?.let {
                        Log.w(TAG, "wake failed", it)
                        WakeTileService.step(address, WakeTilePlan.Step.Failed, app.plainErrorText(it.message))
                    }
                    woke = result.isSuccess
                }
                sender.join(TimeUnit.SECONDS.toMillis(SEND_SECONDS))
                if (sender.isAlive) WakeTileService.step(address, WakeTilePlan.Step.Failed, null)
            } finally {
                pending.finish()
            }
            if (!woke) return@thread
            // Best effort past the broadcast: whether the computer now answers.
            WakeTileService.step(address, WakeTilePlan.Step.Waiting)
            val until = System.currentTimeMillis() + WakeTilePlan.WATCH_MS
            while (System.currentTimeMillis() < until) {
                if (Waker.answers(host.address, host.controlPort)) {
                    WakeTileService.step(address, WakeTilePlan.Step.Awake)
                    return@thread
                }
                Thread.sleep(WATCH_EVERY_MS)
            }
            WakeTileService.step(address, WakeTilePlan.Step.NoAnswer)
        }
    }

    companion object {
        private const val TAG = "WakeTile"
        const val EXTRA_ADDRESS = "address"
        private const val SEND_SECONDS = 8L
        private const val WATCH_EVERY_MS = 2_000L
    }
}
