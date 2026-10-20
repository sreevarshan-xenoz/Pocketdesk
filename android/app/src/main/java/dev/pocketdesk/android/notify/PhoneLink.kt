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
    fun target(ctx: Context): HostStore.Host? {
        // A computer added, removed or paired is seen at once (N7 review).
        val generation = HostStore.generation.get()
        if (generation != seenGeneration) {
            recentTarget.forget()
            seenGeneration = generation
        }
        return recentTarget.get(SystemClock.elapsedRealtime()) {
            HostStore.hosts(ctx).firstOrNull { HostStore.pairedDesktopId(ctx, it.address) != null }
        }
    }

    /** The saved computers' change count [recentTarget] was last checked at. */
    @Volatile
    private var seenGeneration = -1

    /**
     * The answer, kept a few seconds (N7): the background timers ask every
     * few seconds, and each answer read and parsed the saved computers; a
     * computer added or paired is seen within that time.
     */
    private val recentTarget = Recent<HostStore.Host?>(ttlMs = 5_000L)

    /** Send `packetType` with [body] now (on the worker thread). Returns whether it went. */
    fun sendNow(ctx: Context, packetType: String, body: JSONObject): Boolean {
        // L8: the other computers picked get a copy, whatever becomes of this.
        copyToOthers(ctx, packetType, body)
        if (SystemClock.elapsedRealtime() < pausedUntilMs) return false
        val host = target(ctx) ?: return false
        val send = {
            if (packetType == "kdeconnect.notification") {
                // Asked to be answered (C3): one written into a link that has
                // died unseen is not counted as sent, so it waits.
                RustCore.sendNotificationAcked(host.address, host.controlPort, body.toString()).mapCatching { outcome ->
                    check(NotificationDelivery.sent(outcome)) { "the computer did not say it had it" }
                }
            } else {
                RustCore.sendPhonePacket(host.address, host.controlPort, packetType, body.toString())
            }
        }
        var sent = send()
        // Away from home, a one-off connection cannot reach the computer:
        // open the control link, over the internet when it must (T10a), and
        // send on it.
        if (sent.isFailure && RustCore.controlPeer() == null) {
            val wan = HostStore.wanIdentity(ctx, host.address)
            // A failed connect still comes back as a success envelope: what
            // counts is whether a link is now up.
            if (wan != null) {
                RustCore.connectToPeer(host.address, host.controlPort, wan)
                if (RustCore.controlPeer() != null) sent = send()
            }
        }
        return sent.onFailure {
            Log.d(TAG, "not sent to ${host.address}, pausing a minute: ${it.message}")
            pausedUntilMs = SystemClock.elapsedRealtime() + PAUSE_MS
        }.isSuccess
    }

    /**
     * The copies for the other computers (L8), on a worker of their own so a
     * computer out of reach never holds up the phone's own; each paused a
     * minute after it fails, and dropped then (they are copies, not waited).
     */
    private val others: ThreadPoolExecutor =
        ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(40), ThreadPoolExecutor.DiscardOldestPolicy())

    private val othersPausedUntil = java.util.concurrent.ConcurrentHashMap<String, Long>()

    /** What was copied lately, so one sent again after a wait (C3) is copied once. */
    private val copied = object : LinkedHashMap<String, Unit>(64, 0.75f, false) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Unit>?) = size > 200
    }

    private fun copyToOthers(ctx: Context, packetType: String, body: JSONObject) {
        val picked = AlsoToPrefs.picked(ctx)
        if (picked.isEmpty()) return
        val hosts = HostStore.hosts(ctx)
        val targets = AlsoTo.targets(
            packetType,
            hosts.map { it.address },
            { HostStore.pairedDesktopId(ctx, it) != null },
            picked,
            target(ctx)?.address,
        )
        if (targets.isEmpty()) return
        synchronized(copied) {
            if (copied.put(AlsoTo.key(packetType, body), Unit) != null) return
        }
        val copy = AlsoTo.copyFor(body).toString()
        targets.mapNotNull { address -> hosts.firstOrNull { it.address == address } }.forEach { host ->
            others.execute {
                val now = SystemClock.elapsedRealtime()
                if (now < (othersPausedUntil[host.address] ?: 0L)) return@execute
                val sent = if (packetType == "kdeconnect.notification") {
                    RustCore.sendNotificationAcked(host.address, host.controlPort, copy).map { }
                } else {
                    RustCore.sendPhonePacket(host.address, host.controlPort, packetType, copy)
                }
                sent.onFailure {
                    Log.d(TAG, "a copy was not sent to ${host.address}, pausing a minute: ${it.message}")
                    othersPausedUntil[host.address] = SystemClock.elapsedRealtime() + PAUSE_MS
                }
            }
        }
    }
}
