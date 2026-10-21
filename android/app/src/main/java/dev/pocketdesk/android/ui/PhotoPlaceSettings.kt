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
 * asks for the media location permission. The row says when that was taken
 * away since.
 */
@Composable
fun PhotoPlaceSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    // Read again on return: the permission may have changed in Android's settings.
    val resumes = rememberResumes()
    val on = (version + resumes).let { PhotoPlacePrefs.on(context) }
    val granted = (version + resumes).let { PhotoPlacePrefs.granted(context) }

    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            PhotoPlacePrefs.setOn(context, ok)
            version++
        }

    fun setOn(value: Boolean) {
        if (value && !granted) {
            ask.launch(PhotoPlacePrefs.PERMISSION)
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
