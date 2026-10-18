package dev.pocketdesk.android.notify

import android.app.Notification
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import android.os.Process
import android.provider.Settings
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.edit
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
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
    private const val KEY_ANSWERS = "computers_answer"

    /** How long a notification may wait for the computer, in minutes (C3): the choices, the default first. */
    val WAIT_CHOICES = listOf(60, 10, 0)

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun enabled(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ENABLED, false)

    fun setEnabled(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_ENABLED, on) }
    }

    /** The apps whose notifications go to the computer. */
    /** Whether the computer at [address] has said it read a notification (C3): its silence then means a dead link. */
    fun computerAnswers(ctx: Context, address: String): Boolean =
        address in prefs(ctx).getStringSet(KEY_ANSWERS, emptySet()).orEmpty()

    fun noteComputerAnswers(ctx: Context, address: String) {
        val now = prefs(ctx).getStringSet(KEY_ANSWERS, emptySet()).orEmpty()
        if (address !in now) prefs(ctx).edit { putStringSet(KEY_ANSWERS, now + address) }
    }

    /** How long a notification that could not be sent waits for the computer, in minutes (C3). */
    fun waitMinutes(ctx: Context): Int =
        prefs(ctx).getInt(KEY_WAIT, WAIT_CHOICES.first()).takeIf { it in WAIT_CHOICES } ?: WAIT_CHOICES.first()

    fun setWaitMinutes(ctx: Context, minutes: Int) {
        prefs(ctx).edit { putInt(KEY_WAIT, minutes) }
    }

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
    private var answerConnectMs = 0L
    private val answerTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /**
     * The computer browsing this phone's shared folders (T10g), on a 2 s tick
     * of its own: a slow folder or a connect must not hold up answers.
     */
    private val phoneFiles by lazy { dev.pocketdesk.android.files.PhoneFiles(applicationContext) }
    private val phoneFilesTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /** The computer ringing this phone (N1), on a 2 s tick of its own: its connect can take seconds. */
    private val ring by lazy { dev.pocketdesk.android.ring.BackgroundRing(applicationContext) }
    private val ringTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    /** New screenshots to the computer (T10h), on a 2 s tick of their own: a send takes seconds. */
    private val screenshots by lazy { dev.pocketdesk.android.files.Screenshots(applicationContext) }
    private val screenshotsTicker = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    override fun onCreate() {
        super.onCreate()
        rule = NotificationForwardRule(packageName)
        RustCore.start(this)
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
        ringTicker.scheduleWithFixedDelay(
            { runCatching { ring.tick() }.onFailure { Log.w("PhoneNotifications", "ring tick failed", it) } },
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

    override fun onDestroy() {
        answerTicker.shutdownNow()
        phoneFilesTicker.shutdownNow()
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
        worker.execute { handle(sbn, rankingVisibility) }
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        closedKeys.add(sbn.key)
        synchronized(answerable) { answerable.remove(sbn.key) }
        worker.execute {
            // Closed on the phone: closed on the computer too (N5c), but only
            // one the computer was shown. What of it still waits is dropped.
            rule.removed(sbn.key)
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
        return PhoneLink.sendNow(this, "kdeconnect.notification", out)
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
        val offer = NotificationActionRule.signature(offered, reply != null)
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
        if (out.id in closedKeys) return
        // Sent now, or behind what waits; one given up on is forgotten.
        outbox.maxAgeMs = PhoneNotificationPrefs.waitMinutes(this) * 60_000L
        outbox.post(out.id, note.pkg, body, SystemClock.elapsedRealtime(), ::sendable, ::sendBody)
            .forEach { rule.removed(it) }
    }

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

    /** Run the computer's answers that pass [NotificationActionRule.decide]. */
    private fun runAnswers() {
        val own = PhoneLink.target(this)
        // Answers come on the control link: keep one while answering is on.
        val answering = Prefs.answerNotifications(this) && PhoneNotificationPrefs.enabled(this)
        if (answering && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - answerConnectMs >= 60_000L) {
                answerConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, dev.pocketdesk.android.HostStore.wanIdentity(this, own.address))
            }
        }
        val answers = RustCore.takeNotificationAnswers()
        if (answers.isEmpty()) return
        for (answer in answers) {
            val entry = synchronized(answerable) { answerable[answer.key] }
            val offered =
                when {
                    answer.message != null -> entry?.reply != null
                    else -> entry?.actions?.containsKey(answer.action) == true
                }
            var decision =
                NotificationActionRule.decide(
                    settingOn = answering,
                    ownComputer = NotificationActionRule.fromOwnComputer(answer.host, answer.port, own?.address, own?.controlPort),
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
        }
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

    private fun appLabel(pkg: String): String =
        runCatching { packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString() }
            .getOrDefault(pkg)


}

/** A notification's buttons by title, and the button that takes a reply (T10c). */
private class Answerable(val actions: Map<String, Notification.Action>, val reply: Notification.Action?)

/** Answerable notifications remembered, at most. */
private const val MAX_ANSWERABLE = 100
