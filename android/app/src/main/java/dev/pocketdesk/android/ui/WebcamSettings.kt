package dev.pocketdesk.android.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R

/**
 * Settings "Let the computer use my camera" (F6 part 2): off by default;
 * on, it asks for the camera permission. The computer's ask comes on the
 * link the notification listener keeps, so without notification access it
 * says so. The camera itself opens only from the ask's notification, and
 * only while the cast's notification shows.
 */
@Composable
fun WebcamSetting() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.webcam(context) }
    val granted =
        (version + resumes).let {
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED
        }
    val access = (version + resumes).let {
        dev.pocketdesk.android.notify.PhoneNotificationPrefs.accessGranted(context)
    }
    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            Prefs.setWebcam(context, ok)
            version++
        }
    val summary = when {
        on && !granted -> R.string.settings_webcam_needs_camera
        on && !access -> R.string.settings_webcam_needs_access
        else -> R.string.settings_webcam_summary
    }
    SettingSwitch(
        stringResource(R.string.settings_webcam),
        stringResource(summary),
        on && granted,
        { want ->
            if (want && !granted) {
                ask.launch(Manifest.permission.CAMERA)
            } else {
                Prefs.setWebcam(context, want)
                version++
            }
        },
        warning = on && (!granted || !access),
    )
    if (on && granted && !access) AllowNotificationAccess()
}
