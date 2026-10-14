package dev.pocketdesk.android.notify

import android.app.Notification
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/**
 * The phone's notifications on the desktop (N5a): whether they are shared,
 * which apps are left out, and the apps that have been seen to notify (the
 * Settings list). Off until the user turns it on.
 */
object PhoneNotificationPrefs {
    private const val PREFS = "pocketdesk-phone-notifications"
    private const val KEY_ENABLED = "enabled"
    private const val KEY_EXCLUDED = "excluded"
    private const val KEY_SEEN = "seen"
    private const val MAX_SEEN = 100

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun enabled(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ENABLED, false)

    fun setEnabled(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_ENABLED, on) }
    }

    fun excluded(ctx: Context): Set<String> = prefs(ctx).getStringSet(KEY_EXCLUDED, emptySet()).orEmpty().toSet()

    fun setExcluded(ctx: Context, pkg: String, excluded: Boolean) {
        val now = excluded(ctx).toMutableSet().apply { if (excluded) add(pkg) else remove(pkg) }
        prefs(ctx).edit { putStringSet(KEY_EXCLUDED, now) }
    }

    /** The apps seen to notify, package to label, most recent first. */
    fun seen(ctx: Context): List<Pair<String, String>> {
        val json = runCatching { JSONObject(prefs(ctx).getString(KEY_SEEN, "{}") ?: "{}") }.getOrDefault(JSONObject())
        return json.keys().asSequence().map { it to json.optString(it, it) }.toList().reversed()
    }

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

    fun listenerComponent(ctx: Context) = ComponentName(ctx, PhoneNotificationListener::class.java)
}

/**
 * Reads the phone's notifications, once the user grants access, and sends
 * those [NotificationForwardRule] lets through to the most recently used
 * computer this phone is paired with, from one thread. At most 20 wait; a
 * desktop out of reach drops the oldest, not the phone's memory.
 */
class PhoneNotificationListener : NotificationListenerService() {
    private lateinit var rule: NotificationForwardRule
    private val sender =
        ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(20), ThreadPoolExecutor.DiscardOldestPolicy())

    override fun onCreate() {
        super.onCreate()
        rule = NotificationForwardRule(packageName)
        RustCore.start(filesDir)
    }

    override fun onDestroy() {
        sender.shutdownNow()
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        if (!PhoneNotificationPrefs.enabled(this)) return
        val note = runCatching { toNote(sbn) }.getOrNull() ?: return
        if (note.pkg != packageName && !note.ongoing) PhoneNotificationPrefs.recordSeen(this, note.pkg, note.appLabel)
        val out = synchronized(rule) {
            rule.decide(note, enabled = true, excluded = PhoneNotificationPrefs.excluded(this))
        } ?: return
        sender.execute { send(out) }
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        synchronized(rule) { rule.removed(sbn.key) }
    }

    private fun send(out: Outgoing) {
        val host = HostStore.hosts(this).firstOrNull { HostStore.pairedDesktopId(this, it.address) != null } ?: return
        val body =
            JSONObject()
                .put("id", out.id)
                .put("app", out.app)
                .put("title", out.title)
                .put("text", out.text)
                .put("time", System.currentTimeMillis().toString())
                .put("isClearable", true)
        RustCore.sendNotification(host.address, host.controlPort, body.toString())
            .onFailure { Log.d(TAG, "not shown on ${host.address}: ${it.message}") }
    }

    private fun toNote(sbn: StatusBarNotification): PostedNote {
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
            visibility = n.visibility,
            publicTitle = public?.getCharSequence(Notification.EXTRA_TITLE)?.toString(),
            publicText = public?.getCharSequence(Notification.EXTRA_TEXT)?.toString(),
        )
    }

    private fun appLabel(pkg: String): String =
        runCatching { packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString() }
            .getOrDefault(pkg)

    private companion object {
        const val TAG = "PhoneNotifications"
    }
}
