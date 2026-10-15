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

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun enabled(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ENABLED, false)

    fun setEnabled(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_ENABLED, on) }
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
        clipboardTicker.scheduleWithFixedDelay(
            { runCatching { clipboard.tick() }.onFailure { Log.w("PhoneNotifications", "clipboard tick failed", it) } },
            5,
            5,
            TimeUnit.SECONDS,
        )
    }

    override fun onDestroy() {
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

    private fun sendBody(body: JSONObject): Boolean = PhoneLink.sendNow(this, "kdeconnect.notification", body)

    private fun handle(sbn: StatusBarNotification, rankingVisibility: Pair<Int, Int>) {
        if (!PhoneNotificationPrefs.enabled(this)) return
        val note = runCatching { toNote(sbn, rankingVisibility) }.getOrNull() ?: return
        if (note.pkg != packageName && !note.ongoing && !note.otherProfile) {
            PhoneNotificationPrefs.recordSeen(this, note.pkg, note.appLabel)
        }
        val out = rule.decide(note, enabled = true, shared = PhoneNotificationPrefs.shared(this)) ?: return
        val body =
            JSONObject()
                .put("id", out.id)
                .put("app", out.app)
                .put("title", out.title)
                .put("text", out.text)
                .put("time", System.currentTimeMillis().toString())
                .put("isClearable", out.clearable)
        if (out.id in closedKeys) return
        // Sent now, or behind what waits; one given up on is forgotten.
        outbox.post(out.id, note.pkg, body, SystemClock.elapsedRealtime(), ::sendable, ::sendBody)
            .forEach { rule.removed(it) }
    }

    /** Send what waited, oldest first, until one fails. */
    private fun sendWaiting() {
        if (outbox.isEmpty()) return
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
            .getOrDefault(false)

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
