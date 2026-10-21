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
        prefs(ctx).edit { putStringSet(KEY_SHARED, now) }
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

    /** Whether Android lets this app read notifications. */
    fun accessGranted(ctx: Context): Boolean =
        ctx.packageName in NotificationManagerCompat.getEnabledListenerPackages(ctx)

    /** Android's page for granting notification access. */
    fun accessSettingsIntent(): Intent =
        Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}

/** How long after a notification went a link is kept for the computer's clears (L1). */
private const val CLEAR_LINK_MS = 10 * 60_000L

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

    /** When a notification last went to the computer (L1). */
    @Volatile
    private var lastSentMs = Long.MIN_VALUE / 2

    /** Clears from the computer (L1), thirty a minute. */
    private val clearBudget = NotificationActionRule.Budget(ClearRule.PER_MINUTE)
    private var answerConnectMs = 0L
    private val answerTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

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
        phoneFilesTicker.shutdownNow()
        recentPhotosTicker.shutdownNow()
        ringTicker.shutdownNow()
        screenshotsTicker.shutdownNow()
        screenshots.close()
        clipboardTicker.shutdownNow()
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
        worker.execute { handle(sbn, rankingVisibility) }
    }

    /** D2: the phone's Do Not Disturb changed; the computer is told on the ring's thread. */
    override fun onInterruptionFilterChanged(interruptionFilter: Int) {
        ringTicker.execute { runCatching { quiet.filterChanged() } }
    }

    /** Whether Android lets [sbn] through its Do Not Disturb; true when unknown. */
    private fun matchesInterruptionFilter(sbn: StatusBarNotification): Boolean {
        val ranking = Ranking()
        if (!runCatching { currentRanking.getRanking(sbn.key, ranking) }.getOrDefault(false)) return true
        return ranking.matchesInterruptionFilter()
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        closedKeys.add(sbn.key)
        synchronized(answerable) { answerable.remove(sbn.key) }
        worker.execute {
            // Closed on the phone: closed on the computer too (N5c), but only
            // one the computer was shown. What of it still waits is dropped.
            rule.removed(sbn.key)
            // L8: a copy's cancel, whatever the own computer saw of it.
            PhoneLink.copyCancel(this, sbn.key)
            if (outbox.closed(sbn.key) && PhoneNotificationPrefs.enabled(this)) {
                PhoneLink.sendNow(this, "kdeconnect.notification", JSONObject().put("id", sbn.key).put("isCancel", true))
            }
        }
    }

    /** Whether a waiting notification may still go: not closed, forwarding and its app still on. */
    private fun sendable(id: String, pkg: String): Boolean =
        id !in closedKeys && PhoneNotificationPrefs.enabled(this) && pkg in PhoneNotificationPrefs.shared(this)

    /**
     * Send [body]; one that waited says for how long, in whole seconds, so
     * the computer can mark it late (C3). The phone's monotonic clock
     * measures it, so the two clocks need not agree.
     */
    private fun sendBody(body: JSONObject, waitedMs: Long): Boolean {
        val out = if (waitedMs >= 1_000) JSONObject(body.toString()).put("waited", waitedMs / 1_000) else body
        val sent = PhoneLink.sendNow(this, "kdeconnect.notification", out)
        if (sent) lastSentMs = SystemClock.elapsedRealtime()
        // Its app's icon reached this computer (K3 review).
        if (sent) iconOf.remove(body)?.let { iconsSent.add(iconKey(it)) }
        return sent
    }

    private fun handle(sbn: StatusBarNotification, rankingVisibility: Pair<Int, Int>) {
        if (!PhoneNotificationPrefs.enabled(this)) return
        val note = runCatching { toNote(sbn, rankingVisibility) }.getOrNull() ?: return
        if (note.pkg != packageName && !note.ongoing && !note.otherProfile) {
            PhoneNotificationPrefs.recordSeen(this, note.pkg, note.appLabel)
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
        val offer = NotificationActionRule.signature(offered, reply != null) + ConversationRule.digest(thread?.toString())
        val out = rule.decide(note, enabled = true, shared = PhoneNotificationPrefs.shared(this), offer = offer)
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
        if (out.full) offerAnswers(sbn.key, body, offered, actions, reply) else synchronized(answerable) { answerable.remove(sbn.key) }
        // K3: the app's icon once, and its picture with the setting on;
        // whatever an app put in its extras never stops the listener.
        runCatching { addPictures(sbn, note.pkg, body, out.full) }
        // Only with the notification in full.
        if (out.full) thread?.let { body.put("messages", it) }
        if (out.id in closedKeys) return
        // Sent now, or behind what waits; one given up on is forgotten.
        outbox.maxAgeMs = PhoneNotificationPrefs.waitMinutes(this) * 60_000L
        // L8: the other computers picked get a copy now, whatever the own one is doing.
        PhoneLink.copy(this, body)
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
        // L8 part 2: links to the other computers picked, while they may
        // answer, or clear within ten minutes of a notification (as the own
        // link, L1 review: else a link to each all day).
        PhoneLink.keepOthers(this, wanted = answering || clearsWanted)
        if ((answering || callsOn || clearsWanted) && own != null && RustCore.controlPeer() == null) {
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
        // L6: commands for the phone's automation apps, outside a session.
        if (dev.pocketdesk.android.automation.Commands.sessions.get() == 0) {
            dev.pocketdesk.android.automation.Commands.take(this) { host, port ->
                own != null && own.address == host && own.controlPort == port
            }
        }
        val answers = RustCore.takeNotificationAnswers()
        if (answers.isEmpty()) return
        for (answer in answers) {
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
        )
    }

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

/** Answerable notifications remembered, at most. */
private const val MAX_ANSWERABLE = 100
