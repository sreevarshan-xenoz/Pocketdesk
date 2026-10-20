package dev.pocketdesk.android.ui

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
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
import dev.pocketdesk.android.cast.CastSoundRule

/**
 * Settings "The phone's sound with its screen" (K5): off by default; on, it
 * asks for the microphone permission, which Android requires to capture
 * other apps' sound. Not offered before Android 10.
 */
@Composable
fun CastSoundSetting() {
    if (Build.VERSION.SDK_INT < CastSoundRule.MIN_SDK) return
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.castSound(context) }
    val granted =
        (version + resumes).let {
            ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED
        }
    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
            Prefs.setCastSound(context, ok)
            version++
        }
    SettingSwitch(
        stringResource(R.string.settings_cast_sound),
        stringResource(if (on && !granted) R.string.settings_cast_sound_needs_mic else R.string.settings_cast_sound_summary),
        on && granted,
        { want ->
            if (want && !granted) {
                ask.launch(Manifest.permission.RECORD_AUDIO)
            } else {
                Prefs.setCastSound(context, want)
                version++
            }
        },
        warning = on && !granted,
    )
}
