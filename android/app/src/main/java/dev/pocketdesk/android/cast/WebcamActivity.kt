package dev.pocketdesk.android.cast

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R

/**
 * Opened from [WebcamAsk]'s notification (F6 part 2): with the camera
 * permission (asked here if it is not held), the camera starts as the
 * computer's, in [CastService] with its notification showing. Shows
 * nothing of its own.
 */
class WebcamActivity : ComponentActivity() {
    private val askCamera = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) {
            start()
        } else {
            Toast.makeText(this, R.string.camera_denied, Toast.LENGTH_LONG).show()
        }
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WebcamAsk.withdraw(this)
        // The switch turned off since the ask: nothing opens.
        if (!Prefs.webcam(this) || intent.getStringExtra(WebcamAsk.EXTRA_ADDRESS).isNullOrEmpty()) {
            finish()
            return
        }
        if (savedInstanceState != null) return
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            start()
            finish()
        } else {
            askCamera.launch(Manifest.permission.CAMERA)
        }
    }

    private fun start() {
        CastService.startWebcam(
            this,
            intent.getStringExtra(WebcamAsk.EXTRA_ADDRESS).orEmpty(),
            intent.getIntExtra(WebcamAsk.EXTRA_PORT, 0),
            WebcamAsk.Ask(
                intent.getBooleanExtra(WebcamAsk.EXTRA_FRONT, false),
                intent.getIntExtra(WebcamAsk.EXTRA_HEIGHT, 720),
                intent.getIntExtra(WebcamAsk.EXTRA_FPS, 30),
            ),
        )
    }
}
