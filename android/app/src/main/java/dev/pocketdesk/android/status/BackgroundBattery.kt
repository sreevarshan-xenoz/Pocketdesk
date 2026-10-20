package dev.pocketdesk.android.status

import android.content.Context
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink

/**
 * The phone's battery to its computer without a session (research item
 * 11), on the notification listener's tick: as [BatteryRule] says, while
 * "Share the phone's state with the computer" is on, and only on a control link
 * another background feature keeps up (the clipboard, files, answers,
 * folders). It never dials one of its own, so turning on nothing else costs
 * nothing.
 */
class BackgroundBattery(private val ctx: Context) {
    private var last: BatteryRule.Report? = null
    private var lastMs = 0L

    fun tick() {
        if (!Prefs.shareBattery(ctx)) return
        val own = PhoneLink.target(ctx) ?: return
        if (RustCore.controlPeer() != (own.address to own.controlPort)) {
            // A new link is told again at once: the computer may have restarted.
            last = null
            return
        }
        // A session's own BatteryEffect reports it.
        if (RustCore.isStreamingActive) return
        // Only what decides a send is read each tick (L9 review); storage
        // and bars for a report that goes.
        val light = readBattery(ctx, full = false) ?: return
        val nowMs = SystemClock.elapsedRealtime()
        if (!BatteryRule.shouldSend(last, lastMs, light, nowMs)) return
        val now = readBattery(ctx) ?: light
        // On the link alone: if it has just gone, nothing is dialed for this.
        val sent = RustCore.sendPhonePacketOnLink(own.address, own.controlPort, BATTERY, BatteryRule.body(now))
        if (sent.isSuccess) {
            last = now
            lastMs = nowMs
        } else {
            Log.d("BackgroundBattery", "Battery not sent: ${sent.exceptionOrNull()?.message}")
        }
    }
}
