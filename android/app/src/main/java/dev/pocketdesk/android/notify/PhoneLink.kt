package dev.pocketdesk.android.notify

import android.content.Context
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
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

    /** The first paired computer that said the phone is not paired with it, if any. */
    fun refuser(ctx: Context): HostStore.Host? =
        HostStore.hosts(ctx).firstOrNull { HostStore.pairedDesktopId(ctx, it.address) != null && RustCore.unpairedBy(it.address) }

    /** What the bridge says of a send to a computer that unpaired this phone (BB2). */
    const val UNPAIRED_BY = "unpaired this phone"

    /** The computer the phone sends to. */
    fun target(ctx: Context): HostStore.Host? {
        // A computer added, removed or paired is seen at once (N7 review).
        val generation = HostStore.generation.get()
        if (generation != seenGeneration) {
            recentTarget.forget()
            seenGeneration = generation
        }
        return recentTarget.get(SystemClock.elapsedRealtime()) {
            pick(HostStore.hosts(ctx), { HostStore.pairedDesktopId(ctx, it) }, RustCore::unpairedBy)
        }
    }

    /**
     * The phone's own computer among [hosts], most recent first: the first
     * paired one that has not said the phone is not paired with it (BB2: one
     * that unpaired the phone kept the place, and a second, still paired,
     * got nothing).
     */
    fun pick(
        hosts: List<HostStore.Host>,
        pairedId: (String) -> String?,
        unpairedBy: (String) -> Boolean,
    ): HostStore.Host? = hosts.firstOrNull { pairedId(it.address) != null && !unpairedBy(it.address) }

    /** The saved computers' change count [recentTarget] was last checked at. */
    @Volatile
    private var seenGeneration = -1

    /**
     * The answer, kept a few seconds (N7): the background timers ask every
     * few seconds, and each answer read and parsed the saved computers; a
     * computer added or paired is seen within that time.
     */
    private val recentTarget = Recent<HostStore.Host?>(ttlMs = 5_000L)

    /**
     * Send `packetType` with [body] now (on the worker thread). Returns
     * whether it went. With [to], only while the own computer is the one
     * with that paired id (BC4 review: texts kept for one went to the next).
     */
    fun sendNow(ctx: Context, packetType: String, body: JSONObject, to: String? = null): Boolean {
        if (SystemClock.elapsedRealtime() < pausedUntilMs) return false
        val host = target(ctx) ?: return false
        if (to != null && HostStore.pairedDesktopId(ctx, host.address) != to) return false
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
            // That computer said the phone is not paired (BB2): the next
            // paired one is the phone's own at once, not after a pause.
            if (it.message?.contains(UNPAIRED_BY) == true) {
                Log.d(TAG, "${host.address} unpaired this phone; its own computer is looked for again")
                recentTarget.forget()
                return@onFailure
            }
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
        // N7: one to a computer not in use, while another is, says so.
        // A ringing call rings everywhere, as M6 never holds one.
        val on = Prefs.toastWhereWorking(ctx) && !CallRule.isCall(body.optString("id"))
        val elsewhere = { host: HostStore.Host ->
            on && ToastWhere.marks(ToastWhere.key(host.address, host.controlPort), RustCore.inUseAnswers(), true)
        }
        // L8 part 2: a computer linked may answer, so its copy keeps them.
        sendCopies(targets, linked = body.toString(), oneOff = copy.toString(), elsewhere = elsewhere)
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

    /** The targets, kept a few seconds (as [target] is, N7): the answers tick asks every 2 s. */
    private val recentTargets = Recent<Pair<Pair<Int, Set<String>>, List<HostStore.Host>>>(ttlMs = 5_000L)

    private fun copyTargets(ctx: Context): List<HostStore.Host> {
        val picked = AlsoToPrefs.picked(ctx)
        if (picked.isEmpty()) return emptyList()
        val key = HostStore.generation.get() to picked
        val kept = recentTargets.get(SystemClock.elapsedRealtime()) { key to readTargets(ctx, picked) }
        if (kept.first == key) return kept.second
        recentTargets.forget()
        return recentTargets.get(SystemClock.elapsedRealtime()) { key to readTargets(ctx, picked) }.second
    }

    private fun readTargets(ctx: Context, picked: Set<String>): List<HostStore.Host> {
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
    private fun sendCopies(
        targets: List<HostStore.Host>,
        linked: String,
        oneOff: String,
        elsewhere: (HostStore.Host) -> Boolean = { false },
    ) {
        targets.forEach { host ->
            others.execute {
                // N7: decided as it goes, by what each computer said lately.
                val marked = elsewhere(host)
                val mark = { json: String ->
                    if (marked) JSONObject(json).put(ToastWhere.ELSEWHERE, true).toString() else json
                }
                val linked = mark(linked)
                val oneOff = mark(oneOff)
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

    /** The links' own thread: dialing must never hold up the answers tick (L8 part 2 review). */
    private val linker: ThreadPoolExecutor =
        ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(1), ThreadPoolExecutor.DiscardPolicy())

    /**
     * Keep a link to each other computer picked (L8 part 2), so it may answer
     * and clear the notifications it is sent, while [wanted] (answering on,
     * or clearing within ten minutes of a notification, as the own link,
     * L1); close the rest first, so a computer swapped in finds room. A
     * failed link is tried again a minute later. Called from the listener's
     * answers tick; the work is on its own thread, skipped while busy.
     */
    fun keepOthers(ctx: Context, wanted: Boolean) {
        linker.execute {
            val targets = if (wanted) copyTargets(ctx) else emptyList()
            val keep = targets.map { it.address to it.controlPort }
            linkedOthers = RustCore.keepExtra(keep)
            val now = SystemClock.elapsedRealtime()
            var dialed = false
            targets.forEach { host ->
                if ((host.address to host.controlPort) in linkedOthers) return@forEach
                if (now - (othersTriedAt[host.address] ?: Long.MIN_VALUE / 2) < 60_000L) return@forEach
                othersTriedAt[host.address] = now
                dialed = true
                RustCore.connectExtra(host.address, host.controlPort)
                    .onFailure { Log.d(TAG, "no link to ${host.address}: ${it.message}") }
            }
            if (dialed) linkedOthers = RustCore.keepExtra(keep)
        }
    }

    /** Whether [host]:[port] is another computer of this phone's, linked now (L8 part 2). */
    fun linkedOther(host: String, port: Int): Boolean = (host to port) in linkedOthers
}
