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
import dev.pocketdesk.android.files.PhotoPlacePrefs

/**
 * Settings "Photos keep where they were taken" (M5): off by default; on, it
 * asks for photo access and the media location together. The row says when
 * they were taken away since. Not shown before Android 10.
 */
@Composable
fun PhotoPlaceSection() {
    // Android 9 and older take nothing out (M5 review): nothing to turn on.
    if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q) return
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    // Read again on return: the permission may have changed in Android's settings.
    val resumes = rememberResumes()
    val on = (version + resumes).let { PhotoPlacePrefs.on(context) }
    val granted = (version + resumes).let { PhotoPlacePrefs.granted(context) }

    // Both at once (M5 review): Android grants the media location only with
    // photo access, and says nothing when asked for it alone.
    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            PhotoPlacePrefs.setOn(context, PhotoPlacePrefs.granted(context))
            version++
        }

    fun setOn(value: Boolean) {
        if (value && !granted) {
            ask.launch(PhotoPlacePrefs.permissions)
        } else {
            PhotoPlacePrefs.setOn(context, value)
            version++
        }
    }

    SettingSwitch(
        stringResource(R.string.photo_place),
        stringResource(
            when {
                on && !granted -> R.string.photo_place_needs_permission
                on -> R.string.photo_place_on_summary
                else -> R.string.photo_place_off_summary
            },
        ),
        on && granted,
        ::setOn,
        warning = on && !granted,
    )
}
