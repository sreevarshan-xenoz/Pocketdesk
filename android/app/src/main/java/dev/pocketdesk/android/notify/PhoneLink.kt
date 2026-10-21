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
     * minute after it fails, and a copy dropped then (copies do not wait).
     */
    private val others: ThreadPoolExecutor =
        ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(100), ThreadPoolExecutor.DiscardOldestPolicy())

    private val othersPausedUntil = java.util.concurrent.ConcurrentHashMap<String, Long>()

    /** The notifications copied lately, by id, so each one's cancel is copied too (L8 review). */
    private val copiedIds = object : LinkedHashMap<String, Unit>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Unit>?) = size > 500
    }

    /**
     * Copy a notification the phone has just made ready ([body], filtered
     * as the own computer's is) to the other computers picked: taken where
     * it is made, not where it is sent, so a copy goes at once while the own
     * computer is away (L8 review).
     */
    fun copy(ctx: Context, body: JSONObject) {
        val targets = copyTargets(ctx)
        if (targets.isEmpty()) return
        val copy = JSONObject(body.toString())
        AlsoTo.ANSWERS.forEach { copy.remove(it) }
        synchronized(copiedIds) { copiedIds[body.optString("id")] = Unit }
        // L8 part 2: a computer linked may answer, so its copy keeps them.
        sendCopies(targets, linked = body.toString(), oneOff = copy.toString())
    }

    /** Copy the cancel of notification [id], if a copy of it went (L8 review: whatever the own computer saw). */
    fun copyCancel(ctx: Context, id: String) {
        val known = synchronized(copiedIds) { copiedIds.remove(id) != null }
        if (!known) return
        val targets = copyTargets(ctx)
        if (targets.isEmpty()) return
        val cancel = JSONObject().put("id", id).put("isCancel", true).toString()
        sendCopies(targets, linked = cancel, oneOff = cancel)
    }

    private fun copyTargets(ctx: Context): List<HostStore.Host> {
        val picked = AlsoToPrefs.picked(ctx)
        if (picked.isEmpty()) return emptyList()
        val hosts = HostStore.hosts(ctx)
        val addresses = AlsoTo.targets(
            hosts.map { AlsoTo.Saved(it.address, HostStore.pairedDesktopId(ctx, it.address)) },
            picked,
            target(ctx)?.address,
        )
        return addresses.mapNotNull { address -> hosts.firstOrNull { it.address == address } }
    }

    /**
     * Send to each of [targets]: [linked] on its link (L8 part 2), else
     * [oneOff] the way the own computer's notifications go (C3's acked
     * send; a plain phone packet refuses notifications, cancels included).
     */
    private fun sendCopies(targets: List<HostStore.Host>, linked: String, oneOff: String) {
        targets.forEach { host ->
            others.execute {
                if ((host.address to host.controlPort) in linkedOthers &&
                    RustCore.sendOnExtra(host.address, host.controlPort, "kdeconnect.notification", linked).isSuccess
                ) {
                    return@execute
                }
                if (SystemClock.elapsedRealtime() < (othersPausedUntil[host.address] ?: 0L)) return@execute
                val sent = RustCore.sendNotificationAcked(host.address, host.controlPort, oneOff).map { }
                sent.onFailure {
                    Log.d(TAG, "a copy was not sent to ${host.address}, pausing a minute: ${it.message}")
                    othersPausedUntil[host.address] = SystemClock.elapsedRealtime() + PAUSE_MS
                }
            }
        }
    }

    /** The other computers linked now (L8 part 2), as `(address, control port)`. */
    @Volatile
    var linkedOthers: Set<Pair<String, Int>> = emptySet()
        private set

    /** When each other computer's link was last tried. */
    private val othersTriedAt = java.util.concurrent.ConcurrentHashMap<String, Long>()

    /**
     * Keep a link to each other computer picked (L8 part 2), so it may answer
     * and clear the notifications it is sent, while [wanted] (answering or
     * clearing on); close the rest. A failed link is tried again a minute
     * later. Called from the listener's answers tick.
     */
    fun keepOthers(ctx: Context, wanted: Boolean) {
        val targets = if (wanted) copyTargets(ctx) else emptyList()
        val now = SystemClock.elapsedRealtime()
        targets.forEach { host ->
            if ((host.address to host.controlPort) in linkedOthers) return@forEach
            if (now - (othersTriedAt[host.address] ?: Long.MIN_VALUE / 2) < 60_000L) return@forEach
            othersTriedAt[host.address] = now
            RustCore.connectExtra(host.address, host.controlPort)
                .onFailure { Log.d(TAG, "no link to ${host.address}: ${it.message}") }
        }
        linkedOthers = RustCore.keepExtra(targets.map { it.address to it.controlPort })
    }

    /** Whether [host]:[port] is another computer of this phone's, linked now (L8 part 2). */
    fun linkedOther(host: String, port: Int): Boolean = (host to port) in linkedOthers
}
