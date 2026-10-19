package dev.pocketdesk.android.files

import android.Manifest
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.clip.ClipboardShared
import dev.pocketdesk.android.service.TransferService
import dev.pocketdesk.android.ui.ClipboardImages
import dev.pocketdesk.android.ui.ImageClipRule
import java.io.File
import kotlin.concurrent.thread

/**
 * Opened from [CameraAsk]'s notification (F3): the camera app takes one
 * photo into the app's cache, and the photo goes to the computer that
 * asked, as a file and, with the switch on, onto its clipboard. Shows
 * nothing of its own.
 */
class CameraActivity : ComponentActivity() {
    private var address = ""
    private var port = 0
    private var photo: Uri? = null

    private val take = registerForActivityResult(ActivityResultContracts.TakePicture()) { taken ->
        val uri = photo
        if (taken && uri != null) send(uri) else Log.i(TAG, "no photo was taken")
        finish()
    }

    private val askCamera = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) {
            launchCamera()
        } else {
            Toast.makeText(this, R.string.camera_denied, Toast.LENGTH_LONG).show()
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        address = intent.getStringExtra(CameraAsk.EXTRA_ADDRESS).orEmpty()
        port = intent.getIntExtra(CameraAsk.EXTRA_PORT, 0)
        CameraAsk.withdraw(this)
        if (address.isEmpty() || port <= 0) {
            finish()
            return
        }
        if (savedInstanceState != null) {
            photo = savedInstanceState.getString(KEY_PHOTO)?.let(Uri::parse)
            return
        }
        // The app holds the camera permission (its QR scanner), so Android
        // asks it to be granted before another app's camera may be used.
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCamera()
        } else {
            askCamera.launch(Manifest.permission.CAMERA)
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        photo?.let { outState.putString(KEY_PHOTO, it.toString()) }
    }

    private fun launchCamera() {
        val dir = File(cacheDir, "camera").apply { mkdirs() }
        CameraAsk.sweep(dir, System.currentTimeMillis())
        val file = File(dir, "photo-${System.currentTimeMillis()}.jpg")
        val uri = FileProvider.getUriForFile(this, ClipboardImages.authority(this), file)
        photo = uri
        runCatching { take.launch(uri) }.onFailure {
            Log.w(TAG, "no camera app", it)
            Toast.makeText(this, R.string.camera_none, Toast.LENGTH_LONG).show()
            finish()
        }
    }

    private fun send(uri: Uri) {
        val app = applicationContext
        val host = address
        val hostPort = port
        runCatching { TransferService.start(app, host, hostPort, listOf(uri)) }
            .onFailure { Log.w(TAG, "the photo was not sent to the computer", it) }
        if (Prefs.cameraClipboard(app)) {
            thread(name = "camera-clipboard") {
                val png = ClipboardImages.pngOf(app, uri) ?: return@thread
                // Named before it goes, so a session sync does not fetch it back.
                ClipboardShared.sentImageId = ImageClipRule.idOf(png)
                ClipboardImages.withStaged(app, png) { staged -> RustCore.sendClipboardImage(host, hostPort, staged.absolutePath) }
                    .onFailure { Log.w(TAG, "the photo did not reach the computer's clipboard", it) }
            }
        }
    }

    private companion object {
        const val TAG = "Pocketdesk"
        const val KEY_PHOTO = "photo"
    }
}
