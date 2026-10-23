package dev.pocketdesk.android.notify

import android.app.Notification
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.drawable.Drawable
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Parcelable
import android.os.SystemClock
import android.os.Process
import android.provider.Settings
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.edit
import androidx.core.graphics.createBitmap
import androidx.core.graphics.drawable.toDrawable
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * The phone's notifications on the desktop (N5a): whether they are shared,
 * which apps are switched on (none until the user picks them), and the apps
 * that have been seen to notify (the Settings list). Off until turned on.
 */
object PhoneNotificationPrefs {
    private const val PREFS = "pocketdesk-phone-notifications"
    private const val KEY_ENABLED = "enabled"
    private const val KEY_SHARED = "shared"
    private const val KEY_SEEN = "seen"
    private const val MAX_SEEN = 100
    private const val KEY_WAIT = "wait_minutes"

    /** How long a notification may wait for the computer, in minutes (C3): the choices, the default first. */
    val WAIT_CHOICES = listOf(60, 10, 0)

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun enabled(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ENABLED, false)

    fun setEnabled(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_ENABLED, on) }
    }

    /** How long a notification that could not be sent waits for the computer, in minutes (C3). */
    fun waitMinutes(ctx: Context): Int =
        prefs(ctx).getInt(KEY_WAIT, WAIT_CHOICES.first()).takeIf { it in WAIT_CHOICES } ?: WAIT_CHOICES.first()

    fun setWaitMinutes(ctx: Context, minutes: Int) {
        prefs(ctx).edit { putInt(KEY_WAIT, minutes) }
    }

    /** The apps whose notifications go to the computer. */
    fun shared(ctx: Context): Set<String> = prefs(ctx).getStringSet(KEY_SHARED, emptySet()).orEmpty().toSet()

    fun setShared(ctx: Context, pkg: String, shared: Boolean) {
        val now = shared(ctx).toMutableSet().apply { if (shared) add(pkg) else remove(pkg) }
        prefs(ctx).edit {
            putStringSet(KEY_SHARED, now)
            // O5 review: an app no longer shared keeps none of its titles;
            // the kinds kept on the phone stay, as the user chose them.
            if (!shared) remove(KEY_KINDS_PREFIX + pkg)
        }
    }

    /** The apps seen to notify, package to label, most recent first. */
    fun seen(ctx: Context): List<Pair<String, String>> {
        val json = runCatching { JSONObject(prefs(ctx).getString(KEY_SEEN, "{}") ?: "{}") }.getOrDefault(JSONObject())
        return json.keys().asSequence().map { it to json.optString(it, it) }.toList().reversed()
    }

    @Synchronized
    fun recordSeen(ctx: Context, pkg: String, label: String) {
        val json = runCatching { JSONObject(prefs(ctx).getString(KEY_SEEN, "{}") ?: "{}") }.getOrDefault(JSONObject())
        if (json.optString(pkg, "") == label) return
        // Re-inserted last, so the order is "most recently seen" last.
        json.remove(pkg)
        json.put(pkg, label)
        while (json.length() > MAX_SEEN) json.remove(json.keys().next())
        prefs(ctx).edit { putString(KEY_SEEN, json.toString()) }
    }

    private const val KEY_KINDS_PREFIX = "kinds:"
    private const val KEY_KEPT_PREFIX = "kinds_kept:"

    /** O5: the kinds [pkg] was seen to post, most recently seen first. */
    fun kinds(ctx: Context, pkg: String): List<NotificationKinds.Kind> = runCatching {
        val array = org.json.JSONArray(prefs(ctx).getString(KEY_KINDS_PREFIX + pkg, "[]") ?: "[]")
        (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            NotificationKinds.Kind(o.getString("id"), o.optString("example"))
        }
    }.getOrDefault(emptyList())

    /** O5: [pkg] posted a notification of kind [channel], titled as [example]. */
    @Synchronized
    fun recordKind(ctx: Context, pkg: String, channel: String, example: String) {
        val next = NotificationKinds.seen(kinds(ctx, pkg), channel, example, keptKinds(ctx, pkg)) ?: return
        val array = org.json.JSONArray()
        next.forEach { array.put(JSONObject().put("id", it.id).put("example", it.example)) }
        prefs(ctx).edit { putString(KEY_KINDS_PREFIX + pkg, array.toString()) }
    }

    /** O5: the kinds of [pkg] kept on the phone. */
    fun keptKinds(ctx: Context, pkg: String): Set<String> =
        prefs(ctx).getStringSet(KEY_KEPT_PREFIX + pkg, emptySet()).orEmpty().toSet()

    @Synchronized
    fun setKindKept(ctx: Context, pkg: String, channel: String, kept: Boolean) {
        val now = keptKinds(ctx, pkg).toMutableSet().apply { if (kept) add(channel) else remove(channel) }
        prefs(ctx).edit { putStringSet(KEY_KEPT_PREFIX + pkg, now) }
    }

    /** Whether Android lets this app read notifications. */
    fun accessGranted(ctx: Context): Boolean =
        ctx.packageName in NotificationManagerCompat.getEnabledListenerPackages(ctx)

    /** Android's page for granting notification access. */
    fun accessSettingsIntent(): Intent =
        Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}

/** How long after a notification went a link is kept for the computer's clears (L1). */
private const val CLEAR_LINK_MS = 10 * 60_000L

/** The channel of "Tap to open" (N3). */
private const val OPEN_CHANNEL = "opens"

/** How long "Tap to open" waits on the phone (N3 review). */
private const val OPEN_OFFER_MS = 10 * 60_000L

/**
 * Reads the phone's notifications, once the user grants access, and sends
 * those [NotificationForwardRule] lets through to the most recently used
 * computer this phone is paired with. All work runs on one thread, with at
 * most 20 tasks waiting (oldest dropped). After a send fails, sends pause
 * for a minute, so a computer out of reach costs no battery; meanwhile the
 * notifications wait in [NotificationOutbox] (in memory only) and go once
 * the computer answers.
 */
class PhoneNotificationListener : NotificationListenerService() {
    private lateinit var rule: NotificationForwardRule
    private val worker get() = PhoneLink.worker

    /** Notifications that could not be sent yet, and what the computer was shown; [worker] only. */
    private val outbox = NotificationOutbox<JSONObject>()

    /**
     * Notifications closed on the phone, noted on the listener's own thread
     * at once: a worker task can be dropped when many arrive together, and
     * one closed meanwhile must not be sent later.
     */
    private val closedKeys: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    /** O5: each notification's kind, so one waiting is held to a kind kept since. */
    private val kindOf = java.util.concurrent.ConcurrentHashMap<String, String>()

    /** Replies from the computer (N5b): Android keeps this service bound, so it hosts them. */
    private val replies by lazy { SmsReplies(applicationContext) }
    private val replyTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /**
     * The computer's clipboard without a session (T10b), on a 5 s tick of its
     * own: its connect can take seconds while the computer is away, and must
     * not hold up text replies.
     */
    private val clipboard by lazy { dev.pocketdesk.android.clip.BackgroundClipboard(applicationContext) }
    private val clipboardTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /** The battery without a session (research item 11), on the clipboard's ticker, over the link those keep. */
    private val battery by lazy { dev.pocketdesk.android.status.BackgroundBattery(applicationContext) }

    /** Files the computer sends with no session open (T10d), on the clipboard's ticker. */
    private val files by lazy { dev.pocketdesk.android.service.BackgroundFiles(applicationContext) }

    /**
     * The buttons and reply of each notification sent in full while answers
     * are on (T10c), by key; the newest [MAX_ANSWERABLE]. Guarded by itself.
     */
    private val answerable = LinkedHashMap<String, Answerable>()
    private val answerBudget = NotificationActionRule.Budget()

    /** Each live update last sent (N4), by key. */
    private val liveSent = HashMap<String, LiveRule.Sent>()

    /** A live update held back by the gap, looked at again then (N4). */
    private val liveLooks = HashMap<String, java.util.concurrent.ScheduledFuture<*>>()
    private val liveTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /** The notifications sent in full offering to be opened (N3): only these are. */
    private val openable: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    /** When the computers were last asked whether they are in use (N7). */
    private var inUseAskedMs = Long.MIN_VALUE / 2

    /** The asks' own thread (N7 review), one at a time, a busy one skipped. */
    private val inUseAsker = java.util.concurrent.ThreadPoolExecutor(
        1,
        1,
        0,
        TimeUnit.MILLISECONDS,
        java.util.concurrent.ArrayBlockingQueue(1),
        java.util.concurrent.ThreadPoolExecutor.DiscardPolicy(),
    )

    /** When a notification last went to the computer (L1). */
    @Volatile
    private var lastSentMs = Long.MIN_VALUE / 2

    /** Clears from the computer (L1), thirty a minute. */
    private val clearBudget = NotificationActionRule.Budget(ClearRule.PER_MINUTE)
    private var answerConnectMs = 0L
    private val answerTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /**
     * M1: keeps the link's foreground service in step and notices a freeze,
     * on a tick of its own that does nothing slow, so a gap between its runs
     * is the process stopped, never a dial that took long.
     */
    private val keeperTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
    private var lastKeeperUptime: Long? = null
    private var keeperTicks = 0

    /**
     * The computer browsing this phone's shared folders (T10g), on a 2 s tick
     * of its own: a slow folder or a connect must not hold up answers.
     */
    private val phoneFiles by lazy { dev.pocketdesk.android.files.PhoneFiles(applicationContext) }

    /** The computer asking for this phone's recent photos (C7), on a 2 s tick of its own: thumbnails take seconds. */
    private val recentPhotos by lazy { dev.pocketdesk.android.files.RecentPhotos(applicationContext) }
    private val recentPhotosTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
    private val phoneFilesTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /** The computer ringing this phone (N1), on a 2 s tick of its own: its connect can take seconds. */
    private val ring by lazy { dev.pocketdesk.android.ring.BackgroundRing(applicationContext) }
    private val ringTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /**
     * The computer muting this phone (D2), on the ring's tick: both keep the
     * link the same way, and a mute is as rare as a ring.
     */
    private val quiet by lazy {
        BackgroundQuiet(
            applicationContext,
            { runCatching { currentInterruptionFilter }.getOrDefault(QuietPlan.UNKNOWN) },
            { filter -> runCatching { requestInterruptionFilter(filter) } },
        )
    }

    /** Ringing calls on the computer (F5). */
    private val calls by lazy { CallWatch(applicationContext) }

    /** New screenshots to the computer (T10h), on a 2 s tick of their own: a send takes seconds. */
    private val screenshots by lazy { dev.pocketdesk.android.files.Screenshots(applicationContext) }
    private val screenshotsTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
    // N2: the phone's music without a session.
    private val media by lazy { dev.pocketdesk.android.media.BackgroundMedia(applicationContext) }

    // N5: the computer's music in this phone's media controls.
    private val computerMedia by lazy { dev.pocketdesk.android.media.ComputerMedia(applicationContext) }

    override fun onCreate() {
        super.onCreate()
        rule = NotificationForwardRule(packageName)
        RustCore.start(this)
        // F5: ringing calls, watched on the main thread's looper.
        calls.start()
        // On a thread of its own: connecting can take seconds while the
        // computer is away, and must not hold up notifications.
        replyTicker.scheduleWithFixedDelay(
            {
                runCatching { replies.tick() }.onFailure { Log.w("PhoneNotifications", "reply tick failed", it) }
                // Notifications that waited (the phone asleep, the computer away) go now.
                worker.execute { sendWaiting() }
            },
            5,
            5,
            TimeUnit.SECONDS,
        )
        answerTicker.scheduleWithFixedDelay(
            {
                runCatching { runAnswers() }.onFailure { Log.w("PhoneNotifications", "answers failed", it) }
            },
            2,
            2,
            TimeUnit.SECONDS,
        )
        keeperTicker.scheduleWithFixedDelay(
            { runCatching { keepLinked() }.onFailure { Log.w("PhoneNotifications", "keeper tick failed", it) } },
            0,
            2,
            TimeUnit.SECONDS,
        )
        phoneFilesTicker.scheduleWithFixedDelay(
            { runCatching { phoneFiles.tick() }.onFailure { Log.w("PhoneNotifications", "phone files failed", it) } },
            2,
            2,
            TimeUnit.SECONDS,
        )
        recentPhotosTicker.scheduleWithFixedDelay(
            { runCatching { recentPhotos.tick() }.onFailure { Log.w("PhoneNotifications", "recent photos failed", it) } },
            2,
            2,
            TimeUnit.SECONDS,
        )
        ringTicker.scheduleWithFixedDelay(
            {
                runCatching { ring.tick() }.onFailure { Log.w("PhoneNotifications", "ring tick failed", it) }
                runCatching { quiet.tick() }.onFailure { Log.w("PhoneNotifications", "quiet tick failed", it) }
            },
            2,
            2,
            TimeUnit.SECONDS,
        )
        screenshotsTicker.scheduleWithFixedDelay(
            { runCatching { screenshots.tick() }.onFailure { Log.w("PhoneNotifications", "screenshots failed", it) } },
            2,
            2,
            TimeUnit.SECONDS,
        )
        clipboardTicker.scheduleWithFixedDelay(
            {
                runCatching { clipboard.tick() }.onFailure { Log.w("PhoneNotifications", "clipboard tick failed", it) }
                runCatching { files.tick() }.onFailure { Log.w("PhoneNotifications", "files tick failed", it) }
                runCatching { battery.tick() }.onFailure { Log.w("PhoneNotifications", "battery tick failed", it) }
            },
            5,
            5,
            TimeUnit.SECONDS,
        )
        // M4: waits for what the computer types, while its switch and the
        // Accessibility service are on (kept in step on the keeper's tick).
        dev.pocketdesk.android.control.TypeRunner.sync(this)
    }

    // D1: whether the listener runs, for the health check in this process.
    override fun onListenerConnected() {
        super.onListenerConnected()
        dev.pocketdesk.android.doctor.ListenerState.connected(true)
        // Connected after an update's check called it stopped: take its notice
        // back once nothing is wrong (D1 review).
        val app = applicationContext
        Thread {
            runCatching {
                if (dev.pocketdesk.android.doctor.HealthCheck.problems(app).isEmpty()) {
                    dev.pocketdesk.android.doctor.HealthReceiver.cancel(app)
                }
            }
        }.start()
    }

    override fun onListenerDisconnected() {
        dev.pocketdesk.android.doctor.ListenerState.connected(false)
        super.onListenerDisconnected()
    }

    override fun onDestroy() {
        calls.stop()
        answerTicker.shutdownNow()
        inUseAsker.shutdownNow()
        keeperTicker.shutdownNow()
        // Access taken away ends the listener: stop the link's service with it (M1).
        runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(this) }
        phoneFilesTicker.shutdownNow()
        recentPhotosTicker.shutdownNow()
        ringTicker.shutdownNow()
        screenshotsTicker.shutdownNow()
        screenshots.close()
        clipboardTicker.shutdownNow()
        dev.pocketdesk.android.control.TypeRunner.stop(this)
        liveTicker.shutdownNow()
        media.close()
        computerMedia.close()
        clipboard.close()
        replyTicker.shutdownNow()
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        closedKeys.remove(sbn.key)
        // Read the ranking now: it describes the state at this moment.
        val rankingVisibility = rankingVisibility(sbn)
        // D2: whether Do Not Disturb hides it on the phone itself.
        val heldBack = QuietRule.holdBack(
            QuietPrefs.holdBack(this),
            runCatching { currentInterruptionFilter }.getOrDefault(QuietPlan.UNKNOWN),
            matchesInterruptionFilter(sbn),
        )
        if (heldBack) return
        // M6: in hand now, as it comes, not when the worker gets to it (M6
        // review: by then the notification may have woken the screen).
        val inHand = Prefs.quietInHand(this) && inUse().let { (interactive, locked) ->
            InHandRule.marks(true, interactive, locked, InHandRule.remoteInFront)
        }
        worker.execute { handle(sbn, rankingVisibility, inHand) }
    }

    /** D2: the phone's Do Not Disturb changed; the computer is told on the ring's thread. */
    override fun onInterruptionFilterChanged(interruptionFilter: Int) {
        ringTicker.execute { runCatching { quiet.filterChanged() } }
    }

    /** Whether the screen is on, and whether the phone is locked (M6). */
    private fun inUse(): Pair<Boolean, Boolean> {
        val interactive = getSystemService(android.os.PowerManager::class.java)?.isInteractive == true
        val locked = getSystemService(android.app.KeyguardManager::class.java)?.isKeyguardLocked != false
        return interactive to locked
    }

    /** Whether Android lets [sbn] through its Do Not Disturb; true when unknown. */
    private fun matchesInterruptionFilter(sbn: StatusBarNotification): Boolean {
        val ranking = Ranking()
        if (!runCatching { currentRanking.getRanking(sbn.key, ranking) }.getOrDefault(false)) return true
        return ranking.matchesInterruptionFilter()
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        closedKeys.add(sbn.key)
        kindOf.remove(sbn.key)
        // N4: its live update is over.
        synchronized(liveSent) {
            liveSent.remove(sbn.key)
            liveLooks.remove(sbn.key)?.cancel(false)
        }
        synchronized(answerable) { answerable.remove(sbn.key) }
        // N3: no longer opened, and its "Tap to open" goes with it (N3 review).
        if (openable.remove(sbn.key)) {
            runCatching { getSystemService(android.app.NotificationManager::class.java)?.cancel("open", sbn.key.hashCode()) }
        }
        // Closed on the phone: closed on the computer too (N5c), but only
        // one the computer was shown. What of it still waits is dropped.
        worker.execute { withdraw(sbn.key) }
    }

    /** [key] closed on the computers it reached, its waiting copy dropped. On the worker. */
    private fun withdraw(key: String) {
        rule.removed(key)
        // L8: a copy's cancel, whatever the own computer saw of it.
        PhoneLink.copyCancel(this, key)
        if (outbox.closed(key) && PhoneNotificationPrefs.enabled(this)) {
            PhoneLink.sendNow(this, "kdeconnect.notification", JSONObject().put("id", key).put("isCancel", true))
        }
    }

    /** Whether a waiting notification may still go: not closed, forwarding, its app and its kind still on. */
    private fun sendable(id: String, pkg: String): Boolean =
        id !in closedKeys && PhoneNotificationPrefs.enabled(this) && pkg in PhoneNotificationPrefs.shared(this) &&
            kindOf[id]?.let { it in PhoneNotificationPrefs.keptKinds(this, pkg) } != true

    /**
     * Send [body]; one that waited says for how long, in whole seconds, so
     * the computer can mark it late (C3). The phone's monotonic clock
     * measures it, so the two clocks need not agree.
     */
    private fun sendBody(body: JSONObject, waitedMs: Long): Boolean {
        // N7: decided as it goes, by what each computer said lately.
        val own = PhoneLink.target(this)
        val elsewhere = own != null &&
            ToastWhere.marks(ToastWhere.key(own.address, own.controlPort), RustCore.inUseAnswers(), Prefs.toastWhereWorking(this))
        val out = if (waitedMs >= 1_000 || elsewhere) {
            JSONObject(body.toString()).apply {
                if (waitedMs >= 1_000) put("waited", waitedMs / 1_000)
                if (elsewhere) put(ToastWhere.ELSEWHERE, true)
            }
        } else {
            body
        }
        val sent = PhoneLink.sendNow(this, "kdeconnect.notification", out)
        if (sent) lastSentMs = SystemClock.elapsedRealtime()
        // Its app's icon reached this computer (K3 review).
        if (sent) iconOf.remove(body)?.let { iconsSent.add(iconKey(it)) }
        return sent
    }

    private fun handle(sbn: StatusBarNotification, rankingVisibility: Pair<Int, Int>, inHand: Boolean = false) {
        if (!PhoneNotificationPrefs.enabled(this)) return
        val note = runCatching { toNote(sbn, rankingVisibility) }.getOrNull() ?: return
        if (note.pkg != packageName && !note.ongoing && !note.otherProfile) {
            PhoneNotificationPrefs.recordSeen(this, note.pkg, note.appLabel)
            // O5: a shared app's kinds, each with the title it first came
            // with: what the lock screen shows of it, never more, never a
            // code, and none from one that never leaves the phone (O5 review).
            if (note.pkg in PhoneNotificationPrefs.shared(this) && !note.localOnly && !note.groupSummary) {
                val title = when (note.visibility) {
                    NotificationForwardRule.VISIBILITY_PUBLIC -> note.title
                    NotificationForwardRule.VISIBILITY_PRIVATE -> note.publicTitle
                    else -> null
                }?.takeUnless { NotificationForwardRule.looksLikeCode(it) }
                PhoneNotificationPrefs.recordKind(this, note.pkg, note.channel, NotificationKinds.example(title))
            }
        }
        // O5: a kind kept on the phone goes no further, and one of it the
        // computers were shown before it was kept leaves them (O5 review).
        if (note.channel.isNotBlank()) kindOf[sbn.key] = note.channel
        if (note.channel.isNotBlank() && note.channel in PhoneNotificationPrefs.keptKinds(this, note.pkg)) {
            if (rule.wasSent(sbn.key)) worker.execute { withdraw(sbn.key) }
            return
        }
        // N4: an ongoing one's bar or timer, with the switch on; one held
        // back by the gap is looked at again once it passes.
        // Only to a computer that shows them as one card: an older one pops
        // a toast at each update (N4 review).
        val takes = note.ongoing && Prefs.liveUpdates(this) &&
            PhoneLink.target(this)?.let { RustCore.takesLive(it.address) } == true
        val live = if (takes) readLive(sbn, note.ongoing) else null
        // What makes it new to [rule]: its bar when one is due; the last one
        // sent when not, so a change to its buttons still goes (N4 review).
        var liveMarked = live
        var firstLive = false
        if (live != null) {
            val (due, last) = synchronized(liveSent) {
                LiveRule.due(liveSent[sbn.key], live, note.title.orEmpty(), note.text.orEmpty(), SystemClock.elapsedRealtime()) to
                    liveSent[sbn.key]
            }
            when {
                due == null -> liveMarked = last?.live ?: live
                due > 0 -> {
                    lookAgain(sbn.key, due)
                    return
                }
            }
            firstLive = last == null
        }
        // What it offers the computer to answer (T10c), part of what is sent.
        val actions = if (Prefs.answerNotifications(this)) sbn.notification.actions?.toList().orEmpty() else emptyList()
        val infos = actionInfos(actions)
        val offered = NotificationActionRule.offered(infos)
        val reply = NotificationActionRule.replyIndex(infos)?.let { actions[it] }
        // D4: a chat's recent messages, only for a notification the computer
        // may answer, which keeps them only then (D4 review: the toast alone
        // had sent the whole history). Read before deciding whether anything
        // is new: a new message can leave the title and text as they were.
        val thread =
            if (Prefs.answerNotifications(this) && (offered.isNotEmpty() || reply != null)) {
                conversation(sbn.notification)
            } else {
                null
            }
        // N3: a notification sent in full that has a tap action may be
        // opened here from the computer, with the switch on. Not part of
        // what is sent again on a change (N3 review: flipping the switch
        // re-sent every notification): one already sent offers it from its
        // next change.
        val canOpen = Prefs.letComputerOpen(this) && sbn.notification.contentIntent != null
        val offer = NotificationActionRule.signature(offered, reply != null) + ConversationRule.digest(thread?.toString())
        // A live update's bar is part of what makes it new (N4).
        val liveMark = liveMarked?.let { "|live:${it.percent}:${it.indeterminate}:${it.chronometer}:${it.countdown}" }.orEmpty()
        val out = rule.decide(
            note,
            enabled = true,
            shared = PhoneNotificationPrefs.shared(this),
            offer = offer + liveMark,
            live = live != null,
            keptKinds = PhoneNotificationPrefs.keptKinds(this, note.pkg),
            alarmsOn = Prefs.alarmsOnComputer(this),
        )
        if (out == null) {
            // Nothing new to send; an app that posts again often replaces
            // its intents, so the ones kept are refreshed.
            synchronized(answerable) {
                if (sbn.key in answerable) answerable[sbn.key] = Answerable(byTitle(offered, actions), reply)
            }
            return
        }
        val body =
            JSONObject()
                .put("id", out.id)
                .put("app", out.app)
                .put("title", out.title)
                .put("text", out.text)
                .put("time", System.currentTimeMillis().toString())
                .put("isClearable", out.clearable)
        // P5: the computer rings it.
        if (out.alarm) body.put("alarm", true)
        if (out.full) offerAnswers(sbn.key, body, offered, actions, reply) else synchronized(answerable) { answerable.remove(sbn.key) }
        // A redacted one's bar goes nowhere (N4 review): its hidden words
        // would only make it look new.
        if (live != null && !out.full) return
        if (live != null) {
            body.put(
                "live",
                JSONObject()
                    .put("progress", live.progress)
                    .put("max", live.max)
                    .put("indeterminate", live.indeterminate)
                    .put("countdown", live.countdown)
                    .apply {
                        live.chronometer?.let { put("chronometer", it) }
                        live.words?.let { put("words", it) }
                    },
            )
            synchronized(liveSent) {
                liveSent[sbn.key] = LiveRule.Sent(SystemClock.elapsedRealtime(), live, note.title.orEmpty(), note.text.orEmpty())
            }
        }
        if (out.full && canOpen) {
            body.put("canOpen", true)
            openable.add(sbn.key)
        } else {
            openable.remove(sbn.key)
        }
        // K3: the app's icon once, and its picture with the setting on;
        // whatever an app put in its extras never stops the listener.
        // M6: one the computer holds shows no picture there, so none goes
        // (M6 review); its icon still does, which the computer keeps.
        runCatching { addPictures(sbn, note.pkg, body, out.full && !inHand) }
        // Only with the notification in full.
        if (out.full) thread?.let { body.put("messages", it) }
        if (out.id in closedKeys) return
        // M6: came while the phone was in use: the computer keeps it without
        // a pop-up. Decided as it came, so one that waits keeps that moment.
        if (inHand) body.put(InHandRule.FIELD, true)
        // Sent now, or behind what waits; one given up on is forgotten.
        outbox.maxAgeMs = PhoneNotificationPrefs.waitMinutes(this) * 60_000L
        // L8: the other computers picked get a copy now, whatever the own one is doing.
        // A live card's copy goes only with its first (N4 review): another
        // computer would pop a toast at each update.
        if (live == null || firstLive) PhoneLink.copy(this, body)
        outbox.post(out.id, note.pkg, body, SystemClock.elapsedRealtime(), ::sendable, ::sendBody)
            .forEach { rule.removed(it) }
    }

    /** Clear [answer]'s notification here, if [ClearRule] lets it (L1). */
    private fun clear(answer: RustCore.NotificationAnswer, settingOn: Boolean, ownComputer: Boolean) {
        val shown = runCatching { getActiveNotifications(arrayOf(answer.key))?.firstOrNull() }.getOrNull()
        val decision =
            ClearRule.decide(
                settingOn = settingOn,
                ownComputer = ownComputer,
                sent = rule.wasSent(answer.key),
                shown = shown != null,
                clearable = shown?.isClearable == true,
                call = CallRule.isCall(answer.key),
                withinBudget = true,
            ).let {
                if (it == ClearRule.Decision.Clear && !clearBudget.take(SystemClock.elapsedRealtime())) {
                    ClearRule.Decision.TooMany
                } else {
                    it
                }
            }
        if (decision != ClearRule.Decision.Clear) {
            Log.i("PhoneNotifications", "a clear from the computer was not taken: $decision")
            return
        }
        runCatching { cancelNotification(answer.key) }
            .onFailure { Log.w("PhoneNotifications", "a clear failed: ${it.message}") }
    }

    /** What [sbn] shows as a live update (N4), or null when it is not one. */
    private fun readLive(sbn: StatusBarNotification, ongoing: Boolean): LiveRule.Live? {
        val n = sbn.notification
        val e = n.extras
        val live = LiveRule.read(
            ongoing = ongoing,
            own = sbn.packageName == packageName,
            media = e.containsKey(Notification.EXTRA_MEDIA_SESSION),
            call = n.category == Notification.CATEGORY_CALL,
            progress = e.getInt(Notification.EXTRA_PROGRESS),
            max = e.getInt(Notification.EXTRA_PROGRESS_MAX),
            indeterminate = e.getBoolean(Notification.EXTRA_PROGRESS_INDETERMINATE),
            showChronometer = e.getBoolean(Notification.EXTRA_SHOW_CHRONOMETER),
            whenMs = n.`when`,
            countdown = e.getBoolean(Notification.EXTRA_CHRONOMETER_COUNT_DOWN),
        ) ?: return null
        // A timer in this phone's clock and words, which the computer shows
        // as they are (N4 review: words worked out there froze).
        val words = live.chronometer?.let { base ->
            val at = android.text.format.DateFormat.getTimeFormat(this).format(java.util.Date(base))
            getString(if (live.countdown) dev.pocketdesk.android.R.string.live_ends_at else dev.pocketdesk.android.R.string.live_started_at, at)
        }
        return live.copy(words = words)
    }

    /** Look at [key]'s live update again in [delayMs], so its last state is sent (N4). */
    private fun lookAgain(key: String, delayMs: Long) {
        synchronized(liveSent) {
            if (liveLooks[key]?.isDone == false) return
            liveLooks[key] = liveTicker.schedule(
                {
                    synchronized(liveSent) { liveLooks.remove(key) }
                    val sbn = runCatching { getActiveNotifications(arrayOf(key))?.firstOrNull() }.getOrNull()
                    // As if posted now: Do Not Disturb and the phone in
                    // hand are read again (N4 review).
                    if (sbn != null) onNotificationPosted(sbn)
                },
                delayMs,
                java.util.concurrent.TimeUnit.MILLISECONDS,
            )
        }
    }

    /**
     * Open [answer]'s notification here, as a tap on it would (N3), if
     * [OpenRule] lets it: its own tap action sent with Android 14's opt-in
     * to a background start, which the bound Accessibility service allows,
     * else offered in a notification of our own carrying that tap action.
     */
    private fun open(answer: RustCore.NotificationAnswer, ownComputer: Boolean) {
        val shown = runCatching { getActiveNotifications(arrayOf(answer.key))?.firstOrNull() }.getOrNull()
        val tap = shown?.notification?.contentIntent
        val keyguard = getSystemService(android.app.KeyguardManager::class.java)
        val power = getSystemService(android.os.PowerManager::class.java)
        // Unknown counts as locked: an app must never show over the lock
        // screen, nor start on a screen that is off and not locked yet
        // (N3 review: a lock delay, Smart Lock).
        val locked = (keyguard?.isKeyguardLocked ?: true) || power?.isInteractive != true
        val service = dev.pocketdesk.android.control.ControlService.instance != null
        var decision = OpenRule.decide(
            settingOn = Prefs.letComputerOpen(this),
            ownComputer = ownComputer,
            offered = answer.key in openable,
            locked = locked,
            shown = tap != null,
            service = service,
            withinBudget = true,
        )
        if ((decision == OpenRule.Decision.OPEN || decision == OpenRule.Decision.OFFER) &&
            !answerBudget.take(SystemClock.elapsedRealtime())
        ) {
            decision = OpenRule.Decision.TOO_MANY
        }
        val done = when (decision) {
            OpenRule.Decision.OPEN -> runCatching { tap!!.send(this, 0, null, null, null, null, openOptions()) }
                .onFailure { Log.w("PhoneNotifications", "an open failed: ${it.message}") }
                .isSuccess
            OpenRule.Decision.OFFER -> offerOpen(answer.key, tap!!, shown.packageName, answer.host)
            else -> false
        }
        Log.i("PhoneNotifications", "an open from the computer: $decision${if (done) "" else " (not done)"}")
        OpenRule.outcome(decision, done)?.let { tell(answer, it) }
    }

    /**
     * Android 14's opt-in for a tap action sent from the background: this
     * app may start one while its Accessibility service is bound.
     */
    @Suppress("DEPRECATION")
    private fun openOptions(): android.os.Bundle? = when {
        Build.VERSION.SDK_INT >= 36 -> android.app.ActivityOptions.makeBasic()
            .setPendingIntentBackgroundActivityStartMode(android.app.ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS)
            .toBundle()
        Build.VERSION.SDK_INT >= 34 -> android.app.ActivityOptions.makeBasic()
            .setPendingIntentBackgroundActivityStartMode(android.app.ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED)
            .toBundle()
        else -> null
    }

    /** "Tap to open" with [tap], the notification's own tap action; whether it was posted. */
    private fun offerOpen(key: String, tap: android.app.PendingIntent, pkg: String, host: String): Boolean {
        val manager = getSystemService(android.app.NotificationManager::class.java) ?: return false
        if (!manager.areNotificationsEnabled()) return false
        if (manager.getNotificationChannel(OPEN_CHANNEL) == null) {
            manager.createNotificationChannel(
                android.app.NotificationChannel(
                    OPEN_CHANNEL,
                    getString(dev.pocketdesk.android.R.string.channel_opens),
                    android.app.NotificationManager.IMPORTANCE_HIGH,
                ),
            )
        }
        val notification = Notification.Builder(this, OPEN_CHANNEL)
            .setSmallIcon(dev.pocketdesk.android.R.drawable.ic_stat_pocketdesk)
            .setContentTitle(getString(dev.pocketdesk.android.R.string.open_tap_title, appLabel(pkg)))
            .setContentText(getString(dev.pocketdesk.android.R.string.open_tap_text, dev.pocketdesk.android.HostStore.shownName(this, host)))
            .setContentIntent(tap)
            .setAutoCancel(true)
            // An offer is for now (N3 review): gone after ten minutes.
            .setTimeoutAfter(OPEN_OFFER_MS)
            .build()
        return runCatching { manager.notify("open", key.hashCode(), notification) }
            .onFailure { Log.w("PhoneNotifications", "an open was not offered: ${it.message}") }
            .isSuccess
    }

    /**
     * Apps whose icon reached a computer this run (K3), as "computer|app":
     * the computer keeps them. Marked once the notification carrying it was
     * sent, never when it was only put in (K3 review).
     */
    private val iconsSent = java.util.Collections.synchronizedSet(mutableSetOf<String>())

    /** The app whose icon a body carries, until the body is sent. */
    private val iconOf = java.util.Collections.synchronizedMap(java.util.WeakHashMap<JSONObject, String>())

    /** [pkg]'s key in [iconsSent], for the computer notifications go to now. */
    private fun iconKey(pkg: String): String = "${PhoneLink.target(this)?.address}|$pkg"

    /** [body] with [pkg]'s icon the first time, and [sbn]'s picture when it may go (K3). */
    private fun addPictures(sbn: StatusBarNotification, pkg: String, body: JSONObject, full: Boolean) {
        if (NotificationPictures.sendIcon(iconsSent, iconKey(pkg))) {
            runCatching { packageManager.getApplicationIcon(pkg) }.getOrNull()
                ?.let { encoded(it, NotificationPictures.ICON_PX, Bitmap.CompressFormat.PNG) }
                ?.let {
                    body.put("appIcon", it)
                    iconOf[body] = pkg
                }
        }
        if (!NotificationPictures.sendPicture(Prefs.notificationPictures(this), full)) return
        val extras = sbn.notification.extras
        // The untyped reads (the typed ones need Android 13), each checked:
        // an app may put anything under these keys (K3 review).
        @Suppress("DEPRECATION")
        val picture: Drawable? =
            runCatching {
                (extras.getParcelable<Parcelable>(Notification.EXTRA_PICTURE) as? Bitmap)?.toDrawable(resources)
                    ?: if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        (extras.getParcelable<Parcelable>(Notification.EXTRA_PICTURE_ICON) as? Icon)?.loadDrawable(this)
                    } else {
                        null
                    }
                    ?: sbn.notification.getLargeIcon()?.loadDrawable(this)
            }.getOrNull()
        picture
            ?.let { encoded(it, NotificationPictures.PICTURE_PX, Bitmap.CompressFormat.JPEG) }
            ?.let { body.put("picture", it) }
    }

    /** [drawable] drawn at most [max] on its long side, compressed, as base64; a JPEG on white. */
    private fun encoded(drawable: Drawable, max: Int, format: Bitmap.CompressFormat): String? =
        runCatching {
            val (w, h) = NotificationPictures.fit(
                drawable.intrinsicWidth.takeIf { it > 0 } ?: max,
                drawable.intrinsicHeight.takeIf { it > 0 } ?: max,
                max,
            )
            val bitmap = createBitmap(w, h)
            val canvas = Canvas(bitmap)
            if (format == Bitmap.CompressFormat.JPEG) canvas.drawColor(Color.WHITE)
            drawable.setBounds(0, 0, w, h)
            drawable.draw(canvas)
            val out = java.io.ByteArrayOutputStream()
            bitmap.compress(format, 80, out)
            bitmap.recycle()
            android.util.Base64.encodeToString(out.toByteArray(), android.util.Base64.NO_WRAP)
        }.getOrNull()

    /**
     * Offer the computer [sbn]'s buttons and its reply, if any, in [body],
     * and remember them to run the computer's answer (T10c).
     */
    private fun offerAnswers(
        key: String,
        body: JSONObject,
        offered: List<String>,
        actions: List<Notification.Action>,
        reply: Notification.Action?,
    ) {
        synchronized(answerable) {
            answerable.remove(key)
            if (offered.isEmpty() && reply == null) return
            answerable[key] = Answerable(byTitle(offered, actions), reply)
            while (answerable.size > MAX_ANSWERABLE) answerable.remove(answerable.keys.first())
        }
        if (offered.isNotEmpty()) body.put("actions", org.json.JSONArray(offered))
        if (reply != null) body.put("requestReplyId", key)
    }

    /** The offered buttons by title. */
    private fun byTitle(offered: List<String>, actions: List<Notification.Action>): Map<String, Notification.Action> =
        offered.associateWith { title -> actions.first { it.title?.toString() == title } }

    /**
     * What each button is, for [NotificationActionRule]: one that needs the
     * phone unlocked (Android 12+), has no intent, or offers only fixed
     * choices is never the computer's to press.
     */
    private fun actionInfos(actions: List<Notification.Action>): List<NotificationActionRule.ActionInfo> =
        actions.map { a ->
            val inputs = a.remoteInputs
            val freeForm = inputs?.any { it.allowFreeFormInput } == true
            val needsUnlock = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && a.isAuthenticationRequired
            NotificationActionRule.ActionInfo(
                a.title?.toString(),
                takesText = freeForm,
                usable = a.actionIntent != null && !needsUnlock && (inputs.isNullOrEmpty() || freeForm),
            )
        }

    /**
     * Whether an answer from [host]:[port] is from this phone's computers:
     * its own, or another it picked and keeps a link to (L8 part 2).
     */
    private fun fromMine(host: String, port: Int, own: dev.pocketdesk.android.HostStore.Host?): Boolean =
        NotificationActionRule.fromOwnComputer(host, port, own?.address, own?.controlPort) ||
            PhoneLink.linkedOther(host, port)

    /** Run the computer's answers that pass [NotificationActionRule.decide]. */
    /** M1: note a freeze since the last tick, then start or stop the link's service. */
    private fun keepLinked() {
        val uptime = SystemClock.uptimeMillis()
        if (dev.pocketdesk.android.doctor.FreezeWatch.frozen(lastKeeperUptime, uptime)) {
            Log.i("PhoneNotifications", "Paused for ${(uptime - (lastKeeperUptime ?: uptime)) / 1000} s while the phone ran")
            Prefs.setLastFreeze(this, System.currentTimeMillis(), dev.pocketdesk.android.service.LinkKeeperService.isRunning())
        }
        lastKeeperUptime = uptime
        dev.pocketdesk.android.control.TypeRunner.sync(this)
        media.sync()
        // Its inputs (the features, shared folders' grants, the saved
        // computers) cost a read each: every 10 s is soon enough (M1 review).
        if (keeperTicks++ % 5 == 0) dev.pocketdesk.android.service.LinkKeeperService.sync(this)
    }

    private fun runAnswers() {
        val own = PhoneLink.target(this)
        // Answers come on the control link: keep one while answering is on.
        val answering = Prefs.answerNotifications(this) && PhoneNotificationPrefs.enabled(this)
        // F5: a call's answers come on the same link.
        val callsOn = Prefs.callsOn(this)
        // The permission is often granted while the listener runs (F5 review).
        if (callsOn) android.os.Handler(android.os.Looper.getMainLooper()).post { calls.start() }
        calls.retry()
        // L1: the computer's clears come on the same link, taken whenever a
        // link is up; kept up for them only while notifications went in the
        // last ten minutes (L1 review: else every phone sharing them would
        // keep a link all day).
        val clearing = Prefs.letComputerClear(this) && PhoneNotificationPrefs.enabled(this)
        val clearsWanted = clearing && SystemClock.elapsedRealtime() - lastSentMs < CLEAR_LINK_MS
        // N3: opens come on the same link, kept for them as for clears, in
        // the ten minutes after a notification went (N3 review: none came).
        val opensWanted = Prefs.letComputerOpen(this) && PhoneNotificationPrefs.enabled(this) &&
            SystemClock.elapsedRealtime() - lastSentMs < CLEAR_LINK_MS
        // L8 part 2: links to the other computers picked, while they may
        // answer, or clear within ten minutes of a notification (as the own
        // link, L1 review: else a link to each all day).
        // N7: whether each computer is in use, asked on the own link and the
        // others', which are kept for it.
        // Only in the ten minutes after a notification went, as clears are
        // (N7 review: not links and asks all day); the first one after a
        // quiet spell goes to every computer as it would without the switch.
        val toastWhere = Prefs.toastWhereWorking(this) && PhoneNotificationPrefs.enabled(this) &&
            SystemClock.elapsedRealtime() - lastSentMs < CLEAR_LINK_MS
        if (ToastWhere.askDue(toastWhere, SystemClock.elapsedRealtime() - inUseAskedMs)) {
            inUseAskedMs = SystemClock.elapsedRealtime()
            // On a thread of its own (N7 review): a half-open link waits
            // seconds, and the answers tick must not.
            inUseAsker.execute { runCatching { askInUse(own) } }
        }
        PhoneLink.keepOthers(this, wanted = answering || clearsWanted || opensWanted || toastWhere)
        // M4: typing comes on the own computer's link, kept while it may come.
        val typingOn = Prefs.typingFromComputer(this) && dev.pocketdesk.android.control.ControlService.instance != null
        // O2: the computer's screenshots and locks wait on this link too.
        val actionsOn = (Prefs.phoneActionsFromComputer(this) && dev.pocketdesk.android.control.ControlService.instance != null) ||
            // P2: a list of apps needs no service; an open checks for it.
            Prefs.appsFromComputer(this) ||
            // P10: nor does a list of contacts.
            TextPrefs.contactsToComputer(this)
        // N2: the phone's music, while what it reports is current.
        media.tick()
        val mediaWanted = media.linkWanted()
        // N5: the computer's music, while its switch is on.
        computerMedia.tick()
        val computerMediaWanted = computerMedia.linkWanted()
        if ((answering || callsOn || clearsWanted || opensWanted || typingOn || actionsOn || mediaWanted || computerMediaWanted || toastWhere) &&
            own != null && RustCore.controlPeer() == null
        ) {
            val now = SystemClock.elapsedRealtime()
            if (now - answerConnectMs >= 60_000L) {
                answerConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, dev.pocketdesk.android.HostStore.wanIdentity(this, own.address))
            }
        }
        // L4: numbers the computer asks to dial, offered in a notification,
        // unless a session takes them.
        if (!dev.pocketdesk.android.calls.Dials.inSession) {
            dev.pocketdesk.android.calls.Dials.take(this, own, inFront = false)
        }
        // O2: screenshots and locks the computer asks for, taken whatever
        // the switch says, so they never wait for a later turn of it.
        dev.pocketdesk.android.control.PhoneActions.take(this, own)
        // L6: commands for the phone's automation apps, outside a session.
        if (dev.pocketdesk.android.automation.Commands.sessions.get() == 0) {
            dev.pocketdesk.android.automation.Commands.take(this) { host, port ->
                own != null && own.address == host && own.controlPort == port
            }
        }
        val answers = RustCore.takeNotificationAnswers()
        if (answers.isEmpty()) return
        for (answer in answers) {
            // N3: an open, not an answer.
            if (answer.open) {
                val ownComputer = fromMine(answer.host, answer.port, own)
                worker.execute { open(answer, ownComputer) }
                continue
            }
            // L1: a clear, not an answer.
            if (answer.cancel) {
                val ownComputer =
                    fromMine(answer.host, answer.port, own)
                // On the worker, where the forward rule lives (L1 review).
                worker.execute { clear(answer, clearing, ownComputer) }
                continue
            }
            // F5: an answer to a ringing call's note is the call's, not a notification's.
            if (CallRule.isCall(answer.key)) {
                val act = CallRule.decide(
                    key = answer.key,
                    action = answer.action,
                    ringing = calls.ringing,
                    ownComputer = fromMine(answer.host, answer.port, own),
                    on = callsOn,
                    mayDecline = CallWatch.mayDecline(this),
                )
                if (act == CallRule.Act.NotOwnComputer) continue
                val why = calls.act(act)
                tell(answer, NotificationActionRule.Outcome(why == null, why))
                continue
            }
            val entry = synchronized(answerable) { answerable[answer.key] }
            val offered =
                when {
                    answer.message != null -> entry?.reply != null
                    else -> entry?.actions?.containsKey(answer.action) == true
                }
            var decision =
                NotificationActionRule.decide(
                    settingOn = answering,
                    ownComputer = fromMine(answer.host, answer.port, own),
                    known = entry != null,
                    offered = offered,
                    withinBudget = true,
                )
            // Only an answer that would run spends the budget.
            if (decision == NotificationActionRule.Decision.Run && !answerBudget.take(SystemClock.elapsedRealtime())) {
                decision = NotificationActionRule.Decision.TooMany
            }
            if (decision != NotificationActionRule.Decision.Run || entry == null) {
                Log.i("PhoneNotifications", "an answer from the computer was not run: $decision")
                NotificationActionRule.notRun(decision)?.let { tell(answer, it) }
                continue
            }
            val ran =
                runCatching {
                    if (answer.message != null) {
                        val action = entry.reply!!
                        // Only the inputs that take free text are filled.
                        val inputs = action.remoteInputs.filter { it.allowFreeFormInput }.toTypedArray()
                        val results = android.os.Bundle()
                        inputs.forEach { results.putCharSequence(it.resultKey, answer.message) }
                        val fill = Intent()
                        android.app.RemoteInput.addResultsToIntent(inputs, fill, results)
                        action.actionIntent.send(this, 0, fill)
                    } else {
                        entry.actions.getValue(answer.action!!).actionIntent.send()
                    }
                }
            ran.onSuccess { Log.i("PhoneNotifications", "ran the computer's answer") }
                .onFailure { Log.w("PhoneNotifications", "the computer's answer could not run", it) }
            val error = ran.exceptionOrNull()
            tell(answer, NotificationActionRule.ran(error != null, error is android.app.PendingIntent.CanceledException))
        }
    }

    /**
     * Tell the computer [answer] came from how it went (E1), on its link,
     * when the answer named itself; a computer before E1 names none. Another
     * computer of the phone's is told on its own link (L8 part 2).
     */
    private fun tell(answer: RustCore.NotificationAnswer, outcome: NotificationActionRule.Outcome) {
        val id = answer.answerId ?: return
        val body = org.json.JSONObject().put("answerId", id).put("ok", outcome.ok)
        outcome.reason?.let { body.put("reason", it) }
        val sent = if (PhoneLink.linkedOther(answer.host, answer.port)) {
            RustCore.sendOnExtra(answer.host, answer.port, NotificationActionRule.ANSWERED, body.toString())
        } else {
            val own = PhoneLink.target(this) ?: return
            RustCore.sendPhonePacketOnLink(own.address, own.controlPort, NotificationActionRule.ANSWERED, body.toString())
        }
        sent.onFailure { Log.w("PhoneNotifications", "how an answer went was not told", it) }
    }

    /** Ask the own computer and each other one linked whether it is in use (N7); the bridge keeps the answers. */
    private fun askInUse(own: dev.pocketdesk.android.HostStore.Host?) {
        if (own != null && RustCore.controlPeer() == (own.address to own.controlPort)) {
            RustCore.sendPhonePacketOnLink(own.address, own.controlPort, IN_USE_ASK, "{}")
        }
        PhoneLink.linkedOthers.forEach { (host, port) -> RustCore.sendOnExtra(host, port, IN_USE_ASK, "{}") }
    }

    /** Send what waited, oldest first, until one fails. */
    private fun sendWaiting() {
        if (outbox.isEmpty()) return
        outbox.maxAgeMs = PhoneNotificationPrefs.waitMinutes(this) * 60_000L
        outbox.flush(SystemClock.elapsedRealtime(), ::sendable, ::sendBody).forEach { rule.removed(it) }
    }

    /** The channel's and the user's lock-screen overrides for [sbn]. */
    private fun rankingVisibility(sbn: StatusBarNotification): Pair<Int, Int> {
        val ranking = Ranking()
        if (!runCatching { currentRanking.getRanking(sbn.key, ranking) }.getOrDefault(false)) {
            return NotificationForwardRule.NO_OVERRIDE to NotificationForwardRule.NO_OVERRIDE
        }
        val channel = ranking.channel?.lockscreenVisibility ?: NotificationForwardRule.NO_OVERRIDE
        val user =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                ranking.lockscreenVisibilityOverride
            } else {
                NotificationForwardRule.NO_OVERRIDE
            }
        return channel to user
    }

    /** Whether the user set the lock screen to hide sensitive content. */
    private fun systemHidesSensitive(): Boolean =
        runCatching { Settings.Secure.getInt(contentResolver, "lock_screen_allow_private_notifications", 1) == 0 }
            // Unreadable: taken as hiding, so content is never sent the
            // lock screen might hide.
            .getOrDefault(true)

    private fun toNote(sbn: StatusBarNotification, rankingVisibility: Pair<Int, Int>): PostedNote {
        val n = sbn.notification
        val extras = n.extras
        val text = extras.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: extras.getCharSequence(Notification.EXTRA_TEXT)
        val public = n.publicVersion?.extras
        return PostedNote(
            key = sbn.key,
            pkg = sbn.packageName,
            appLabel = appLabel(sbn.packageName),
            ongoing = sbn.isOngoing || n.flags and Notification.FLAG_FOREGROUND_SERVICE != 0,
            groupSummary = n.flags and Notification.FLAG_GROUP_SUMMARY != 0,
            title = extras.getCharSequence(Notification.EXTRA_TITLE)?.toString(),
            text = text?.toString(),
            visibility =
                NotificationForwardRule.effectiveVisibility(
                    n.visibility,
                    rankingVisibility.first,
                    rankingVisibility.second,
                    systemHidesSensitive(),
                ),
            publicTitle = public?.getCharSequence(Notification.EXTRA_TITLE)?.toString(),
            publicText = public?.getCharSequence(Notification.EXTRA_TEXT)?.toString(),
            localOnly = n.flags and Notification.FLAG_LOCAL_ONLY != 0,
            otherProfile = sbn.user != Process.myUserHandle(),
            clearable = sbn.isClearable,
            channel = n.channelId.orEmpty(),
            alarm = AlarmRule.isAlarm(
                n.category,
                sbn.packageName,
                clockPackages(),
                n.flags and Notification.FLAG_NO_CLEAR != 0,
                counting = extras.getBoolean(Notification.EXTRA_SHOW_CHRONOMETER) ||
                    extras.getInt(Notification.EXTRA_PROGRESS_MAX) > 0,
            ),
            clockApp = sbn.packageName in clockPackages(),
        )
    }

    /** The clock apps (P5), read once a run: those that set alarms. */
    private val clocks by lazy {
        listOf(android.provider.AlarmClock.ACTION_SET_ALARM, android.provider.AlarmClock.ACTION_SHOW_ALARMS)
            .flatMap { action ->
                runCatching { packageManager.queryIntentActivities(android.content.Intent(action), 0) }.getOrDefault(emptyList())
            }
            .map { it.activityInfo.packageName }
            .toSet()
    }

    private fun clockPackages(): Set<String> = clocks

    /**
     * A chat notification's recent messages (D4), as [ConversationRule]
     * lets them go, or null for one that is not a chat. A message with no
     * person is the phone owner's own.
     */
    private fun conversation(n: Notification): JSONArray? {
        val style = runCatching {
            androidx.core.app.NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(n)
        }.getOrNull() ?: return null
        val raw = style.messages.map { m ->
            ConversationRule.Raw(
                from = m.person?.name?.toString(),
                text = m.text?.toString(),
                timeMs = m.timestamp,
                // The owner's own: no person (androidx's convention), or the
                // style's user, by key or, for apps that give none, by name.
                mine = m.person == null || m.person == style.user ||
                    m.person?.key?.let { it == style.user.key } == true ||
                    (m.person?.key == null && !style.user.name.isNullOrBlank() && m.person?.name == style.user.name),
            )
        }
        val out = ConversationRule.messages(raw)
        if (out.isEmpty()) return null
        return JSONArray().apply {
            out.forEach { m ->
                put(JSONObject().put("from", m.from).put("text", m.text).put("time", m.timeMs).put("mine", m.mine))
            }
        }
    }

    private fun appLabel(pkg: String): String =
        runCatching { packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString() }
            .getOrDefault(pkg)


}

/** A notification's buttons by title, and the button that takes a reply (T10c). */
private class Answerable(val actions: Map<String, Notification.Action>, val reply: Notification.Action?)

/** Phone → computer: are you in use? (`core::protocol::in_use::ASK`, N7) */
private const val IN_USE_ASK = "kdeconnect.pocketdesk.in_use"

/** Answerable notifications remembered, at most. */
private const val MAX_ANSWERABLE = 100
