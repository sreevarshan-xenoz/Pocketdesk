package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.NotificationActionRule
import org.json.JSONArray
import org.json.JSONObject

/**
 * O2: the screenshots and locks this phone's computer asks for, done
 * through C8's control service when [PhoneActionRule] says so. A
 * screenshot reaches the computer as T10h's new screenshots do.
 */
object PhoneActions {
    private const val TAG = "PhoneActions"

    /** Phone → computer: the apps that open (P2), `core::protocol::phone_action::PHONE_APPS`. */
    private const val PHONE_APPS = "kdeconnect.pocketdesk.phone_apps"

    /** The apps that open from the launcher, by their label, never this one. */
    private fun apps(context: Context): JSONObject {
        val pm = context.packageManager
        val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val seen = mutableSetOf<String>()
        val list = JSONArray()
        runCatching { pm.queryIntentActivities(launcher, 0) }.getOrDefault(emptyList())
            .mapNotNull { info ->
                val pkg = info.activityInfo?.packageName ?: return@mapNotNull null
                pkg to runCatching { info.loadLabel(pm).toString() }.getOrDefault(pkg)
            }
            .filter { (pkg, _) -> pkg != context.packageName && seen.add(pkg) }
            .sortedBy { it.second.lowercase() }
            .take(PhoneActionRule.MAX_APPS)
            .forEach { (pkg, label) -> list.put(JSONObject().put("id", pkg).put("name", label)) }
        return JSONObject().put("apps", list)
    }
    private val budget = NotificationActionRule.Budget(PhoneActionRule.PER_MINUTE)

    /** The apps last listed for the computer: an open takes one of them only (P2 review). */
    @Volatile
    private var listed: Set<String> = emptySet()

    /** A list being made, off the answers tick (P2 review: hundreds of labels read). */
    private val listing = java.util.concurrent.atomic.AtomicBoolean(false)

    /** The channel of "Tap to open", as N3's. */
    private const val OPEN_CHANNEL = "opens"

    /** How long "Tap to open" waits, as N3's. */
    private const val OPEN_OFFER_MS = 10 * 60_000L

    /** Android's opt-in to start an app from the background, as N3 sends its opens. */
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

    /**
     * Open [pkg]: through N3's road while the control service is bound
     * (Android lets the app start one from the background then, with its
     * opt-in), else "Tap to open" for ten minutes. What it did.
     */
    private fun open(context: Context, pkg: String, host: String, service: Boolean): String {
        val intent = context.packageManager.getLaunchIntentForPackage(pkg)?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ?: return "not found"
        val tap = android.app.PendingIntent.getActivity(
            context,
            pkg.hashCode(),
            intent,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        if (service && runCatching { tap.send(context, 0, null, null, null, null, openOptions()) }.isSuccess) return "opened"
        val manager = context.getSystemService(android.app.NotificationManager::class.java) ?: return "not offered"
        if (!manager.areNotificationsEnabled()) return "not offered: notifications are off"
        if (manager.getNotificationChannel(OPEN_CHANNEL) == null) {
            manager.createNotificationChannel(
                android.app.NotificationChannel(
                    OPEN_CHANNEL,
                    context.getString(dev.pocketdesk.android.R.string.channel_opens),
                    android.app.NotificationManager.IMPORTANCE_HIGH,
                ),
            )
        }
        val label = runCatching {
            context.packageManager.getApplicationLabel(context.packageManager.getApplicationInfo(pkg, 0)).toString()
        }.getOrDefault(pkg)
        val notification = android.app.Notification.Builder(context, OPEN_CHANNEL)
            .setSmallIcon(dev.pocketdesk.android.R.drawable.ic_stat_pocketdesk)
            .setContentTitle(context.getString(dev.pocketdesk.android.R.string.open_tap_title, label))
            .setContentText(context.getString(dev.pocketdesk.android.R.string.open_tap_text, HostStore.shownName(context, host)))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setTimeoutAfter(OPEN_OFFER_MS)
            .build()
        return if (runCatching { manager.notify("open-app", pkg.hashCode(), notification) }.isSuccess) "offered" else "not offered"
    }

    /** Take the asks waiting; [own] is the computer the phone works with. */
    fun take(context: Context, own: HostStore.Host?) {
        val asks = RustCore.takePhoneActions()
        if (asks.isEmpty()) return
        val service = ControlService.instance
        for (ask in asks) {
            val action = PhoneActionRule.Action.parse(ask.action) ?: continue
            val ownComputer = own != null && own.address == ask.host && own.controlPort == ask.port
            var decision = PhoneActionRule.decide(
                action = action,
                on = Prefs.phoneActionsFromComputer(context),
                own = ownComputer,
                service = service != null,
                sdk = Build.VERSION.SDK_INT,
                locked = context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked ?: true,
                awake = context.getSystemService(PowerManager::class.java)?.isInteractive ?: false,
                // T10h carries it, to the clipboard or Downloads (O2 review: with
                // both off, it would be taken and go nowhere).
                screenshots = ScreenshotPrefs.on(context) && ScreenshotPrefs.granted(context) &&
                    (ScreenshotPrefs.clipboard(context) || ScreenshotPrefs.save(context)),
                withinBudget = true,
                appsOn = Prefs.appsFromComputer(context),
                listed = ask.app in listed,
            )
            if (decision == PhoneActionRule.Decision.DO && !budget.take(SystemClock.elapsedRealtime())) {
                decision = PhoneActionRule.Decision.TOO_MANY
            }
            if (decision != PhoneActionRule.Decision.DO) {
                Log.i(TAG, "an ask from the computer (${action.word}) was not done: $decision")
                continue
            }
            // P2: the apps listed for the computer, off this tick, or one opened.
            if (action == PhoneActionRule.Action.APPS) {
                if (own == null || !listing.compareAndSet(false, true)) continue
                val app = context.applicationContext
                Thread {
                    try {
                        val body = apps(app)
                        val ids = mutableSetOf<String>()
                        val list = body.getJSONArray("apps")
                        for (i in 0 until list.length()) ids.add(list.getJSONObject(i).getString("id"))
                        listed = ids
                        val sent = RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_APPS, body.toString()).isSuccess
                        Log.i(TAG, if (sent) "the apps were listed for the computer" else "the apps could not be sent")
                    } finally {
                        listing.set(false)
                    }
                }.start()
                continue
            }
            if (action == PhoneActionRule.Action.OPEN) {
                Log.i(TAG, "an app the computer named: ${open(context, ask.app, ask.host, service != null)}")
                continue
            }
            if (service == null) continue
            val done = Build.VERSION.SDK_INT >= PhoneActionRule.MIN_SDK &&
                service.performGlobalAction(
                    if (action == PhoneActionRule.Action.SCREENSHOT) {
                        AccessibilityService.GLOBAL_ACTION_TAKE_SCREENSHOT
                    } else {
                        AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN
                    },
                )
            // What became of it, for the device checks.
            Log.i(TAG, if (done) "a ${action.word} from the computer was done" else "a ${action.word} from the computer failed")
        }
    }
}
