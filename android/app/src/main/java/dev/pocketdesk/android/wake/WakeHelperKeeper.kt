package dev.pocketdesk.android.wake

import android.content.Context
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import org.json.JSONArray

/**
 * Keeps what computers said about wake helpers, as the bridge hands it over:
 * a duty (this phone is a computer's helper: what to wake it with, and for
 * whom) and an info (where a computer's helper is). Both only from a computer
 * this phone paired with, and only when the body names that computer's own
 * id ([WakeHelperRule.keep]); a duty only from this phone's own computer, and
 * only while the switch is on. Nothing here logs a key or an address.
 */
object WakeHelperKeeper {
    private const val TAG = "WakeHelper"

    /**
     * Take and keep what arrived. Returns whether a computer said it had
     * removed this phone as its helper, in which case the switch is turned
     * off here (it stays removed until its owner turns it on again).
     */
    fun poll(ctx: Context): Boolean {
        val array = runCatching { JSONArray(RustCore.takeWakeHelper()) }.getOrNull() ?: return false
        if (array.length() == 0) return false
        val own = PhoneLink.target(ctx)
        val on = Prefs.wakeHelper(ctx)
        var refused = false
        for (i in 0 until array.length()) {
            val o = array.optJSONObject(i) ?: continue
            val body = o.optJSONObject("body") ?: continue
            val saved = HostStore.hosts(ctx).firstOrNull {
                it.address == o.optString("host") && it.controlPort == o.optInt("port")
            } ?: continue
            if (!WakeHelperRule.keep(body.optString("computer"), HostStore.pairedDesktopId(ctx, saved.address))) {
                Log.i(TAG, "a word from a computer that is not the one paired there was dropped")
                continue
            }
            when (o.optString("type")) {
                WakeHelperRule.DUTY -> when {
                    WakeHelperRule.refused(body) -> {
                        HostStore.clearWakeHelperDuty(ctx, saved.address)
                        if (on) refused = true
                    }
                    on && own?.address == saved.address ->
                        HostStore.saveWakeHelperDuty(ctx, saved.address, body.toString())
                }
                WakeHelperRule.INFO -> HostStore.saveWakeHelperInfo(ctx, saved.address, body.toString())
            }
        }
        if (refused) {
            Log.i(TAG, "a computer removed this phone as its wake helper: the switch is off")
            Prefs.setWakeHelper(ctx, false)
            Prefs.setWakeHelperFresh(ctx, false)
            forgetDuties(ctx)
        }
        return refused
    }

    /** The duties this phone serves: its own computer's, when it holds one. */
    fun duties(ctx: Context): List<String> {
        val own = PhoneLink.target(ctx) ?: return emptyList()
        return listOfNotNull(HostStore.wakeHelperDuty(ctx, own.address))
    }

    /** Forget every duty held: the switch went off, or a computer removed this phone. */
    fun forgetDuties(ctx: Context) {
        for (host in HostStore.hosts(ctx)) HostStore.clearWakeHelperDuty(ctx, host.address)
    }
}
