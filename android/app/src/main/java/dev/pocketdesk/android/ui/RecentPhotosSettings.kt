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
import dev.pocketdesk.android.files.RecentPhotoPrefs
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings "Recent photos on the computer" (C7): off by default; on, it asks
 * for photo access. The notification listener answers the computer, so the
 * row says when its access is missing.
 */
@Composable
fun RecentPhotosSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    // Read again on return: access may have been granted in Android's settings.
    val resumes = rememberResumes()
    val on = (version + resumes).let { RecentPhotoPrefs.on(context) }
    val granted = (version + resumes).let { RecentPhotoPrefs.granted(context) }
    val listening = (version + resumes).let { PhoneNotificationPrefs.accessGranted(context) }
    val videos = (version + resumes).let { RecentPhotoPrefs.videos(context) }
    // U5: the ask is said only while it is still to be made.
    val videoAccess = (version + resumes).let { dev.pocketdesk.android.files.CameraPrefs.videosGranted(context) }

    // T4: the camera's videos listed too, asking for video access.
    val askVideos =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            RecentPhotoPrefs.setVideos(context, ok)
            version++
        }

    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            RecentPhotoPrefs.setOn(context, ok)
            version++
        }

    fun setOn(value: Boolean) {
        if (value && !granted) {
            ask.launch(RecentPhotoPrefs.permission)
        } else {
            RecentPhotoPrefs.setOn(context, value)
            version++
        }
    }

    val active = on && granted
    val needsAccess = active && !listening
    SettingSwitch(
        stringResource(R.string.recent_photos_on),
        stringResource(
            when {
                needsAccess -> R.string.recent_photos_needs_access
                on && !granted -> R.string.recent_photos_needs_photos
                else -> R.string.recent_photos_on_summary
            },
        ),
        active,
        ::setOn,
        warning = needsAccess || (on && !granted),
    )
    if (needsAccess) AllowNotificationAccess()
    if (active) {
        SettingSwitch(
            stringResource(R.string.recent_photos_videos),
            if (videoAccess) null else stringResource(R.string.camera_uploads_videos_summary),
            videos,
            {
                if (it && !dev.pocketdesk.android.files.CameraPrefs.videosGranted(context)) {
                    askVideos.launch(dev.pocketdesk.android.files.CameraPrefs.videoPermission)
                } else {
                    RecentPhotoPrefs.setVideos(context, it)
                    version++
                }
            },
            indent = true,
        )
    }
}
