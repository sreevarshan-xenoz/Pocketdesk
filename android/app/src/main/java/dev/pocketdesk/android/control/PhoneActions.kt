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

/**
 * O2: the screenshots and locks this phone's computer asks for, done
 * through C8's control service when [PhoneActionRule] says so. A
 * screenshot reaches the computer as T10h's new screenshots do.
 */
object PhoneActions {
    private const val TAG = "PhoneActions"
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
                screenshots = ScreenshotPrefs.on(context) && ScreenshotPrefs.granted(context),
                withinBudget = true,
            )
            if (decision == PhoneActionRule.Decision.DO && !budget.take(SystemClock.elapsedRealtime())) {
                decision = PhoneActionRule.Decision.TOO_MANY
            }
            if (decision != PhoneActionRule.Decision.DO || service == null) {
                Log.i(TAG, "a ${action.word} from the computer was not done: $decision")
                continue
            }
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
