package dev.pocketdesk.android.cast

import android.app.Activity
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import dev.pocketdesk.android.control.AppOpenRule
import dev.pocketdesk.android.control.PhoneActions

/**
 * Opened from [ScreenForApp]'s notification (Apps part 1): with the phone's
 * "Let the computer open my apps" still on and no screen already shown to
 * that computer, it asks Android for the screen-capture consent once
 * (the system dialog; nothing else is asked), starts the cast, and the app
 * the computer named opens when the cast is up. With a cast already
 * running it only opens the app. Shows nothing of its own.
 */
class ScreenForAppActivity : ComponentActivity() {
    private var address = ""
    private var port = 0
    private var controlPort = 0
    private var pkg = ""

    private val consent = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        if (result.resultCode == Activity.RESULT_OK && data != null) {
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
        if (CastService.castingTo != null) {
            // Another cast (the camera, or sound alone) runs: one at a time.
            finish()
            return
        }
        consent.launch(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent())
    }
}
