package dev.pocketdesk.android.notify

import android.content.Context
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/**
 * What the phone sends the computer on its own: its notifications (N5a) and
 * texts (N5b). One worker thread for all of it, with at most 20 waiting
 * (oldest dropped), to the most recently used computer this phone is paired
 * with. After a send fails, sends pause for a minute, so a computer out of
 * reach costs no battery.
 */
object PhoneLink {
    private const val TAG = "PhoneLink"
    private const val PAUSE_MS = 60_000L

    val worker: ThreadPoolExecutor =
        ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(20), ThreadPoolExecutor.DiscardOldestPolicy())

    /** Until when sends are paused after a failure (elapsed realtime). */
    @Volatile
    private var pausedUntilMs = 0L

    /** The computer the phone sends to. */
    fun target(ctx: Context): HostStore.Host? =
        HostStore.hosts(ctx).firstOrNull { HostStore.pairedDesktopId(ctx, it.address) != null }

    /** Send `packetType` with [body] now (on the worker thread). Returns whether it went. */
    fun sendNow(ctx: Context, packetType: String, body: JSONObject): Boolean {
        if (SystemClock.elapsedRealtime() < pausedUntilMs) return false
        val host = target(ctx) ?: return false
        val sent =
            if (packetType == "kdeconnect.notification") {
                RustCore.sendNotification(host.address, host.controlPort, body.toString())
            } else {
                RustCore.sendPhonePacket(host.address, host.controlPort, packetType, body.toString())
            }
        return sent.onFailure {
            Log.d(TAG, "not sent to ${host.address}, pausing a minute: ${it.message}")
            pausedUntilMs = SystemClock.elapsedRealtime() + PAUSE_MS
        }.isSuccess
    }
}
