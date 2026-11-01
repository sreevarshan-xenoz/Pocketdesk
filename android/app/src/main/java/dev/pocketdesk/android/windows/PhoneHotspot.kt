package dev.pocketdesk.android.windows

import android.content.Context
import android.os.Build
import android.util.Log
import dev.pocketdesk.android.Prefs

/**
 * Let my computers turn on my hotspot: the phone's side of "Use my phone's
 * hotspot". [HotspotRule] decides; this asks Shizuku's helper to start or stop
 * the phone's own hotspot, and turns it off by itself after the owner's number
 * of minutes. The phone's usual hotspot indicator is Android's own.
 */
object PhoneHotspot {
    private const val TAG = "PhoneHotspot"

    private val limiter = HotspotRule.Limiter()

    @Volatile
    private var stopping = false

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

    /** Do an ask [decide] let through, off the caller's thread. */
    fun run(context: Context, ask: HotspotRule.Ask) {
        val app = context.applicationContext
        val on = ask == HotspotRule.Ask.ON
        ShizukuBridge.hotspot(app, on, { dev.pocketdesk.android.cast.CastService.windowIds.isNotEmpty() }) { ok ->
            Log.i(TAG, "the hotspot was asked to turn ${if (on) "on" else "off"}: ${if (ok) "done" else "it did not go through"}")
            if (ok) Prefs.setHotspotOnAt(app, if (on) System.currentTimeMillis() else 0L)
        }
    }

    /**
     * Turn the hotspot off at once if it was turned on from here: the owner turned the switch
     * off, or the time is up. Does nothing for a hotspot the owner turned on themselves.
     */
    fun stopIfOurs(context: Context) {
        if (Prefs.hotspotOnAt(context) <= 0L || stopping) return
        stopping = true
        val app = context.applicationContext
        ShizukuBridge.hotspot(app, false, { dev.pocketdesk.android.cast.CastService.windowIds.isNotEmpty() }) { ok ->
            Log.i(TAG, "the hotspot was turned off by itself: ${if (ok) "done" else "it did not go through"}")
            // Forgotten either way: a hotspot that cannot be reached is not tried every second.
            Prefs.setHotspotOnAt(app, 0L)
            stopping = false
        }
    }

    /** From the listener's tick: turn it off when the owner's minutes have passed since the last ask. */
    fun tick(context: Context, nowMs: Long) {
        if (HotspotRule.offDue(Prefs.hotspotOnAt(context), nowMs, Prefs.hotspotOffMinutes(context))) stopIfOurs(context)
    }
}
