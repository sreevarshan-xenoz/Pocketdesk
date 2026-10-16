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
 * "Share battery with the computer" is on, and only on a control link
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
        if (RustCore.controlPeer() != (own.address to own.controlPort)) return
        val now = readBattery(ctx) ?: return
        val nowMs = SystemClock.elapsedRealtime()
        if (!BatteryRule.shouldSend(last, lastMs, now, nowMs)) return
        val sent = RustCore.sendPhonePacket(own.address, own.controlPort, BATTERY, BatteryRule.body(now))
        if (sent.isSuccess) {
            last = now
            lastMs = nowMs
        } else {
            Log.d("BackgroundBattery", "Battery not sent: ${sent.exceptionOrNull()?.message}")
        }
    }
}
