package dev.pocketdesk.android.cast

import android.app.Activity
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.control.AppOpenRule
import dev.pocketdesk.android.control.ConsentRule
import dev.pocketdesk.android.control.ControlService
import dev.pocketdesk.android.control.PhoneActions

/**
 * Opened from [ScreenForApp]'s notification (Apps part 1): with the phone's
 * "Let the computer open my apps" still on and no screen already shown to
 * that computer, it asks Android for the screen-capture consent once
 * (the system dialog; nothing else is asked), starts the cast, and the app
 * the computer named opens when the cast is up. With a cast already
 * running it only opens the app. Shows nothing of its own.
 *
 * Hands-free: started by Pocketdesk itself ([ScreenForApp.EXTRA_AUTO]) for an
 * open the phone's own computer asked, with "Accept screen sharing for my
 * computers" on, it also arms the control service to press the system's own
 * question ([ConsentRule]) as it launches it. If the service cannot press it
 * (the button is not offered to it, or the screen is laid out differently), the
 * ordinary one-tap notification is offered and the computer is told a tap is
 * needed.
 */
class ScreenForAppActivity : ComponentActivity() {
    private var address = ""
    private var port = 0
    private var controlPort = 0
    private var pkg = ""

    private val consent = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        // The question is answered, by the owner's tap or the press: nothing is waiting any more.
        ControlService.instance?.disarmConsent()
        val data = result.data
        if (result.resultCode == Activity.RESULT_OK && data != null) {
            ScreenForApp.withdraw(this)
            CastService.start(this, address, port, result.resultCode, data)
            PhoneActions.openWhenShown(applicationContext, pkg, address, controlPort)
        }
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ScreenForApp.withdraw(this)
        address = intent.getStringExtra(ScreenForApp.EXTRA_ADDRESS).orEmpty()
        port = intent.getIntExtra(ScreenForApp.EXTRA_PORT, 0)
        controlPort = intent.getIntExtra(ScreenForApp.EXTRA_CONTROL_PORT, 0)
        pkg = intent.getStringExtra(ScreenForApp.EXTRA_APP).orEmpty()
        // The switch turned off, or the computer changed, since the ask:
        // nothing opens (review).
        if (address.isEmpty() || pkg.isEmpty() || PhoneActions.stillRefused(this, address) == AppOpenRule.OFF) {
            finish()
            return
        }
        if (savedInstanceState != null) return
        if (CastService.screenTo == address) {
            // Already shown there: the same capture serves, no new consent.
            PhoneActions.openWhenShown(applicationContext, pkg, address, controlPort)
            finish()
            return
        }
        if (CastService.resume(this, address, port)) {
            // Paused when this computer left, kept for a while: resumed with
            // the consent already given (Hands-free); no new question.
            PhoneActions.openWhenShown(applicationContext, pkg, address, controlPort)
            finish()
            return
        }
        if (CastService.castingTo != null) {
            // Another cast (the camera, or sound alone) runs: one at a time.
            finish()
            return
        }
        val manager = getSystemService(MediaProjectionManager::class.java)
        val auto = intent.getBooleanExtra(ScreenForApp.EXTRA_AUTO, false)
        // Hands-free: from Pocketdesk itself only, and only while the owner's opt-in, the
        // apps switch, the own computer and the control service all still say so.
        val service = ControlService.instance
        val may = auto && ConsentRule.mayArm(
            autoOn = Prefs.autoAcceptScreen(this),
            appsOn = Prefs.appsFromComputer(this),
            own = dev.pocketdesk.android.notify.PhoneLink.target(this)?.address == address,
            service = service != null,
        )
        if (auto && (!may || service == null)) {
            // Not allowed any more: no question is raised; the ordinary one-tap offer stands.
            PhoneActions.consentNeedsTap(applicationContext, HostStore.Host(address, port, controlPort), pkg)
            finish()
            return
        }
        // Android 14 and later: the entire screen of the default display is what an open
        // needs, so the question is raised with no one-app choice at all.
        val question = if (may && Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            manager.createScreenCaptureIntent()
        }
        if (may && service != null) {
            val app = applicationContext
            val host = HostStore.Host(address, port, controlPort)
            // The one activity Android resolves this very question to, and Pocketdesk's own
            // label as Android writes it in the question: only that window is ever pressed.
            // Android names it in the intent itself (an explicit component, which package
            // visibility does not hide); the resolver is the fallback.
            val named = question.component
            val resolved = if (named == null) packageManager.resolveActivity(question, 0)?.activityInfo else null
            val label = applicationInfo.loadLabel(packageManager).toString()
            service.armConsent(
                address,
                label,
                named?.packageName ?: resolved?.packageName,
                named?.className ?: resolved?.name,
            ) { result ->
                // Pressed: the cast follows from the result below. Anything else: a tap is needed.
                if (result != ConsentRule.PRESSED) PhoneActions.consentNeedsTap(app, host, pkg)
            }
        }
        consent.launch(question)
    }
}
