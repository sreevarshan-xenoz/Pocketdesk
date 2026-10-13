package dev.pocketdesk.android.ui

import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import dev.pocketdesk.android.Prefs

/**
 * View-level haptic feedback (C8), gated on [Prefs.haptics]. Goes through
 * View.performHapticFeedback rather than the Vibrator service: no permission
 * needed and the system-wide "touch feedback" setting still governs it.
 */
class LlHaptics internal constructor(private val view: View, private val enabled: Boolean) {
    /** Neutral tap: chips, list picks, the action disc. */
    fun tap() = feed(HapticFeedbackConstants.KEYBOARD_TAP)

    /** State change: mode/pane toggles. */
    fun toggle() = feed(
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) TOGGLE else HapticFeedbackConstants.VIRTUAL_KEY,
    )

    /** Weightier confirmation: session exit/disconnect. */
    fun longPress() = feed(
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            HapticFeedbackConstants.LONG_PRESS
        } else {
            HapticFeedbackConstants.VIRTUAL_KEY
        },
    )

    private fun feed(constant: Int) {
        if (enabled) view.performHapticFeedback(constant)
    }

    companion object {
        // HapticFeedbackConstants.TOGGLE is newer than this project's android.jar;
        // resolve it by name so both compile and the tuned effect on 13+ still work.
        val TOGGLE: Int = runCatching {
            HapticFeedbackConstants::class.java.getField("TOGGLE").getInt(null)
        }.getOrDefault(HapticFeedbackConstants.VIRTUAL_KEY)
    }
}

@Composable
fun rememberLlHaptics(): LlHaptics {
    val view = LocalView.current
    val context = LocalContext.current
    // Subscribe to pref changes so flipping "Haptics" in Settings takes
    // effect immediately, without recreating the screen.
    Prefs.observedVersion()
    return LlHaptics(view, Prefs.haptics(context))
}
