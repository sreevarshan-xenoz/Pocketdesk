package dev.pocketdesk.android.windows

import android.content.Context
import android.os.Build
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink

/**
 * Let my computers turn on my hotspot: the phone's side of "Use my phone's
 * hotspot". [HotspotRule] decides; this asks Shizuku's helper to start or stop
 * the phone's own hotspot, and keeps what it turned on ([Prefs.hotspotRun])
 * until a stop has gone through: it is turned off by the owner's minutes, by
 * a hard cap whatever they say, when the asking computer's link has been gone
 * a couple of minutes or it is no longer paired, and when the owner turns the
 * switch off. A stop that fails is tried again until it works. A hotspot the
 * owner turned on themselves is never recorded, so never turned off from here.
 * The phone's usual hotspot indicator is Android's own.
 */
object PhoneHotspot {
    private const val TAG = "PhoneHotspot"

    private val limiter = HotspotRule.Limiter()
    private val link = HotspotRule.LinkWatch()

    @Volatile
    private var stopping = false

    @Volatile
    private var lastTryMs = 0L

    /** Where hotspots stand now. */
    fun mode(context: Context): HotspotRule.Mode = HotspotRule.mode(
        Build.VERSION.SDK_INT,
        ShizukuBridge.running(),
        ShizukuBridge.permitted(),
        Prefs.hotspotFromComputer(context),
    )

    /** What the computer is told of this feature in the switches report. */
    fun report(context: Context) = HotspotRule.report(
        Build.VERSION.SDK_INT,
        ShizukuBridge.running(),
        ShizukuBridge.permitted(),
        Prefs.hotspotFromComputer(context),
    )

    /** What an ask comes to, without doing it; the rate limit is taken here when it would be done. */
    fun decide(context: Context, ask: HotspotRule.Ask, own: Boolean, nowMs: Long): HotspotRule.Decision {
        val decision = HotspotRule.decide(ask, mode(context), own, withinLimit = true)
        return if (decision == HotspotRule.Decision.DO && !limiter.take(nowMs)) HotspotRule.Decision.TOO_MANY else decision
    }

    private fun windowsOpen() = dev.pocketdesk.android.cast.CastService.windowIds.isNotEmpty()

    /** Do an ask [decide] let through, off the caller's thread. [asker] is the computer's address. */
    fun run(context: Context, ask: HotspotRule.Ask, asker: String) {
        val app = context.applicationContext
        val on = ask == HotspotRule.Ask.ON
        ShizukuBridge.hotspot(app, on, { windowsOpen() }) { result ->
            Log.i(TAG, "the hotspot was asked to turn ${if (on) "on" else "off"}: result $result")
            if (on) {
                // What this ask comes to for the record of what is ours (see HotspotRule.afterOn).
                Prefs.setHotspotRun(app, HotspotRule.afterOn(Prefs.hotspotRun(app), result, asker, System.currentTimeMillis()))
                if (result != HotspotRule.RESULT_FAILED) Prefs.setHotspotStopPending(app, false)
            } else if (result == HotspotRule.RESULT_DONE) {
                Prefs.setHotspotRun(app, null)
            } else if (Prefs.hotspotRun(app) != null) {
                // The computer asked for it off and it did not go: owed, and tried again.
                Prefs.setHotspotStopPending(app, true)
            }
        }
    }

    /**
     * Turn the hotspot off if Pocketdesk turned it on: the owner turned the switch off, or the computer
     * that asked is gone. It is kept (and tried again on each tick) until the stop goes through.
     */
    fun stopIfOurs(context: Context) {
        if (Prefs.hotspotRun(context) == null) return
        Prefs.setHotspotStopPending(context, true)
        tryStop(context.applicationContext, System.currentTimeMillis())
    }

    private fun tryStop(app: Context, nowMs: Long) {
        if (stopping) return
        stopping = true
        lastTryMs = nowMs
        ShizukuBridge.hotspot(app, false, { windowsOpen() }) { result ->
            Log.i(TAG, "the hotspot was asked to turn off by itself: result $result")
            // Forgotten only when it went through: a hotspot that could not be reached is not let go.
            if (result == HotspotRule.RESULT_DONE) Prefs.setHotspotRun(app, null)
            stopping = false
        }
    }

    /** Whether the computer that asked is still one of this phone's paired computers. */
    private fun askerPaired(context: Context, asker: String): Boolean =
        HostStore.hosts(context).any { it.address == asker } &&
            HostStore.pairedDesktopId(context, asker) != null &&
            !RustCore.unpairedBy(asker)

    /**
     * From the listener's tick: say why a hotspot of ours is to go off (the cap, the computer unpaired, the
     * owner's minutes, its link gone) and then keep trying to turn it off until it has.
     */
    fun tick(context: Context, nowMs: Long) {
        val run = Prefs.hotspotRun(context) ?: return
        if (!Prefs.hotspotStopPending(context)) {
            val linked = RustCore.controlPeer()?.first == run.asker
            val lost = link.lostFor(linked, nowMs)
            val why = HotspotRule.stopDue(run, nowMs, Prefs.hotspotOffMinutes(context), askerPaired(context, run.asker), lost)
                ?: return
            Log.i(TAG, "the hotspot is to be turned off: $why")
            Prefs.setHotspotStopPending(context, true)
        }
        if (HotspotRule.retryDue(lastTryMs, nowMs)) tryStop(context.applicationContext, nowMs)
    }
}
