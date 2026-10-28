package dev.pocketdesk.android.control

import android.content.Context
import android.hardware.display.DisplayManager
import android.os.SystemClock
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

/**
 * The phone as another screen for its computer's keyboard and mouse (BK5),
 * kept by the notification listener. It tells the phone's own computer what
 * the phone is: whether "Let my computer's keyboard and mouse use this
 * phone" and the Accessibility service are on, and its screen, as any of it
 * changes, every 15 s while the switch is on, and once as it turns off.
 * While the switch and C8's Accessibility service are on, one thread waits
 * for what the computer sends and does it through the service, as
 * [PointerPlan] says, showing the computer's pointer. Taking is what keeps
 * the bridge answering the computer's pings, so the computer brings its
 * mouse home when this stops.
 */
object PointerRunner {
    private const val TAG = "PointerRunner"

    /** How long one wait lasts; a finger down is looked at sooner. */
    private const val WAIT_MS = 200
    private const val BUSY_WAIT_MS = 20

    /** How often the phone says itself again while the switch is on. */
    private const val REPORT_EVERY_MS = 15_000L

    /** Which thread may act: a stopped one's last batch is not done. */
    @Volatile
    private var generation = 0
    private var running = false

    /** What the computer was last told, to whom and when; null once it was told off. */
    private var said: String? = null
    private var saidTo: Pair<String, Int>? = null
    private var saidAtMs = 0L

    /** Start or stop the thread, and tell the computer, as the switch and the service say: on the listener's ticks. */
    @Synchronized
    fun sync(ctx: Context) {
        val on = Prefs.keyboardMouseFromComputer(ctx)
        if (on && ControlService.instance != null) start(ctx) else stop()
        runCatching { report(ctx, on) }.onFailure { Log.w(TAG, "The screen was not said", it) }
    }

    @Synchronized
    private fun start(ctx: Context) {
        if (running) return
        running = true
        val app = ctx.applicationContext
        val mine = ++generation
        Thread({ loop(app, mine) }, "pocketdesk-pointer").apply {
            isDaemon = true
            start()
        }
    }

    /** The thread stops at its next wait, lifting a finger down and hiding the pointer. */
    @Synchronized
    fun stop() {
        if (!running) return
        running = false
        generation++
    }

    private fun loop(ctx: Context, mine: Int) {
        val pointer = PointerPlan.Pointer()
        val pacer = PointerPlan.Pacer()
        try {
            while (generation == mine) {
                val busy = ControlService.instance?.waiting() == true
                val batch = runCatching { RustCore.sharedInputTake(if (busy) BUSY_WAIT_MS else WAIT_MS) }.getOrNull()
                if (generation != mine) break
                val service = ControlService.instance
                val now = SystemClock.uptimeMillis()
                if (batch != null && service != null) {
                    val own = PhoneLink.target(ctx)
                    val decision = PointerPlan.decide(
                        on = Prefs.keyboardMouseFromComputer(ctx),
                        service = true,
                        own = own != null && own.address == batch.host && own.controlPort == batch.port,
                    )
                    if (decision == PointerPlan.Decision.TAKE) {
                        for (event in events(batch.events)) perform(service, pacer.admit(pointer.take(event, now), now))
                        // A button held still stays down while the computer
                        // sends (its pings, five a second): the review's I2.
                        if (pointer.fingerDown) service.keepPressed()
                    } else {
                        Log.i(TAG, "Not taken: $decision")
                        perform(service, pointer.release())
                    }
                }
                service?.let {
                    perform(it, pointer.tick(now))
                    // A press held still starts, a forgotten finger lifts.
                    it.tick()
                }
            }
        } catch (e: RuntimeException) {
            Log.w(TAG, "The pointer stopped", e)
        } finally {
            ControlService.instance?.let { perform(it, pointer.release()) }
            synchronized(this) {
                if (generation == mine) running = false
            }
        }
    }

    /** A batch's events, each as [PointerPlan.Event]; one that does not read is passed over. */
    private fun events(json: String): List<PointerPlan.Event> {
        val array = runCatching { JSONArray(json) }.getOrNull() ?: return emptyList()
        return (0 until array.length()).mapNotNull { i ->
            val e = array.optJSONObject(i) ?: return@mapNotNull null
            PointerPlan.Event(
                type = e.optString("type"),
                x = e.optInt("x"),
                y = e.optInt("y"),
                button = e.optString("button"),
                action = e.optString("action"),
                dx = e.optInt("dx"),
                dy = e.optInt("dy"),
                text = e.optString("text"),
                key = e.optString("key"),
            )
        }
    }

    /** Do [acts] through the service ([PointerPlan.Pacer] has limited a pointer's moves). */
    private fun perform(service: ControlService, acts: List<PointerPlan.Act>) {
        val refused = { Log.i(TAG, "Not typed: a password field"); Unit }
        for (act in acts) {
            when (act) {
                is PointerPlan.Act.Pointer -> service.pointerAt(act.x, act.y)
                PointerPlan.Act.Hide -> service.hidePointer()
                is PointerPlan.Act.Touch -> service.touch(act.action, act.x, act.y)
                is PointerPlan.Act.Scroll -> service.scroll(act.x, act.y, act.notches)
                // In turn with what is typed, as M4's are.
                is PointerPlan.Act.Global -> service.globalBlind(act.action)
                is PointerPlan.Act.Text -> service.typeBlind(act.text, refused)
                is PointerPlan.Act.Key -> service.keyBlind(act.key, refused)
            }
        }
    }

    /**
     * Tell the phone's own computer what this phone is ([on]: the switch),
     * when it changed, every [REPORT_EVERY_MS] while on, or once as it turns
     * off; again until it went.
     */
    @Synchronized
    private fun report(ctx: Context, on: Boolean) {
        if (!on && said == null) return
        val own = PhoneLink.target(ctx) ?: return
        val to = own.address to own.controlPort
        val json = screen(ctx, on, ControlService.instance != null) ?: return
        val now = SystemClock.elapsedRealtime()
        val due = json != said || to != saidTo || (on && now - saidAtMs >= REPORT_EVERY_MS)
        if (!due) return
        if (RustCore.sharedInputScreen(own.address, own.controlPort, json).isSuccess) {
            said = if (on) json else null
            saidTo = to
            saidAtMs = now
        }
    }

    /** The screen as it is drawn now, in the packet's words. */
    private fun screen(ctx: Context, allows: Boolean, service: Boolean): String? {
        val display = ctx.getSystemService(DisplayManager::class.java)?.getDisplay(Display.DEFAULT_DISPLAY) ?: return null
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        display.getRealMetrics(metrics)
        val density = metrics.density.takeIf { it > 0f } ?: return null
        return JSONObject()
            .put("allows", allows)
            .put("service", service)
            .put("width_px", metrics.widthPixels)
            .put("height_px", metrics.heightPixels)
            .put("width_dp", (metrics.widthPixels / density).roundToInt())
            .put("height_dp", (metrics.heightPixels / density).roundToInt())
            .put("rotation", display.rotation)
            .toString()
    }
}
