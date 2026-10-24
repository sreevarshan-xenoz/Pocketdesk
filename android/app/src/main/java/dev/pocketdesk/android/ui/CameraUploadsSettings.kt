package dev.pocketdesk.android.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.R
import dev.pocketdesk.android.files.CameraPrefs
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings "Send new camera photos" (T3): off by default; on, it asks for
 * photo access, "Videos too" asks for video access, and "Only on Wi-Fi" is
 * on. The notification listener does the work, so the row says when its
 * access is missing.
 */
@Composable
fun CameraUploadsSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    // Read again on return: access may have been granted in Android's settings.
    val resumes = rememberResumes()
    val on = (version + resumes).let { CameraPrefs.on(context) }
    val granted = (version + resumes).let { CameraPrefs.photosGranted(context) }
    val videos = (version + resumes).let { CameraPrefs.videos(context) && CameraPrefs.videosGranted(context) }
    val wifiOnly = (version + resumes).let { CameraPrefs.wifiOnly(context) }
    // U5: the ask is said only while it is still to be made.
    val videoAccess = (version + resumes).let { CameraPrefs.videosGranted(context) }
    val listening = (version + resumes).let { PhoneNotificationPrefs.accessGranted(context) }

    val askPhotos =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            CameraPrefs.setOn(context, ok)
            version++
        }
    val askVideos =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            CameraPrefs.setVideos(context, ok)
            version++
        }

    fun setOn(value: Boolean) {
        if (value && !granted) {
            askPhotos.launch(ScreenshotPrefs.permission)
        } else {
            CameraPrefs.setOn(context, value)
            version++
        }
    }

    val active = on && granted
    val needsAccess = active && !listening
    SettingSwitch(
        stringResource(R.string.camera_uploads_on),
        stringResource(
            when {
                needsAccess -> R.string.screenshots_needs_access
                on && !granted -> R.string.screenshots_needs_photos
                else -> R.string.camera_uploads_on_summary
            },
        ),
        active,
        ::setOn,
        warning = needsAccess || (on && !granted),
    )
    if (needsAccess) AllowNotificationAccess()
    if (active) {
        SettingSwitch(
            stringResource(R.string.camera_uploads_videos),
            if (videoAccess) null else stringResource(R.string.camera_uploads_videos_summary),
            videos,
            {
                if (it && !CameraPrefs.videosGranted(context)) {
                    askVideos.launch(CameraPrefs.videoPermission)
                } else {
                    CameraPrefs.setVideos(context, it)
                    version++
                }
            },
            indent = true,
        )
        SettingSwitch(
            stringResource(R.string.camera_uploads_wifi),
            stringResource(R.string.camera_uploads_wifi_summary),
            wifiOnly,
            {
                CameraPrefs.setWifiOnly(context, it)
                version++
            },
            indent = true,
        )
    }
}
