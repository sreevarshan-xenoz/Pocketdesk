package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.NotificationActionRule
import android.content.Intent
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
            )
            if (decision == PhoneActionRule.Decision.DO && !budget.take(SystemClock.elapsedRealtime())) {
                decision = PhoneActionRule.Decision.TOO_MANY
            }
            if (decision != PhoneActionRule.Decision.DO) {
                Log.i(TAG, "a ${action.word} from the computer was not done: $decision")
                continue
            }
            // P2: the apps listed for the computer, or one opened.
            if (action == PhoneActionRule.Action.APPS) {
                val sent = own != null && RustCore.sendPhonePacketOnLink(own.address, own.controlPort, PHONE_APPS, apps(context).toString()).isSuccess
                Log.i(TAG, if (sent) "the apps were listed for the computer" else "the apps could not be sent")
                continue
            }
            if (action == PhoneActionRule.Action.OPEN) {
                val intent = context.packageManager.getLaunchIntentForPackage(ask.app)?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                // From the bound service, which Android lets start an app from the background.
                val opened = intent != null && service != null && runCatching { service.startActivity(intent) }.isSuccess
                Log.i(TAG, if (opened) "an app was opened for the computer" else "an app the computer named was not opened")
                continue
            }
            if (service == null) continue
            val done = Build.VERSION.SDK_INT >= PhoneActionRule.MIN_SDK &&
                service.performGlobalAction(
                    when (action) {
                        PhoneActionRule.Action.SCREENSHOT -> AccessibilityService.GLOBAL_ACTION_TAKE_SCREENSHOT
                        PhoneActionRule.Action.LOCK -> AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN
                    },
                )
            // What became of it, for the device checks.
            Log.i(TAG, if (done) "a ${action.word} from the computer was done" else "a ${action.word} from the computer failed")
        }
    }
}
