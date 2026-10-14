package dev.pocketdesk.android

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.edit

/**
 * App-level UI preferences (theme mode, dynamic color, screen behavior,
 * PIN reveal, haptics), persisted separately from the per-desktop records
 * in [HostStore]. [observedVersion] is Compose snapshot state: screens that
 * must restyle on a pref change read it, and every setter bumps it.
 */
@Stable
object Prefs {
    private const val PREFS = "pocketdesk-ui"
    private const val KEY_THEME_MODE = "theme_mode"
    private const val KEY_DYNAMIC_COLOR = "dynamic_color"
    private const val KEY_KEEP_SCREEN_ON = "keep_screen_on"
    private const val KEY_AUTO_RECONNECT = "auto_reconnect"
    private const val KEY_BACKGROUND_REMINDER = "background_reminder"
    private const val KEY_REVEAL_PIN = "reveal_pin"
    private const val KEY_HAPTICS = "haptics"
    private const val KEY_CONNECT_AFTER_WAKE = "connect_after_wake"
    private const val KEY_OFFER_WAKE = "offer_wake"
    private const val KEY_SHOW_DRIVING = "show_driving"
    private const val KEY_DESKTOP_SOUND = "desktop_sound"
    private const val KEY_TRACKPAD_SENSITIVITY = "trackpad_sensitivity"

    /** The trackpad speed slider's range and default (F3). */
    const val TRACKPAD_SENSITIVITY_MIN = 0.5f
    const val TRACKPAD_SENSITIVITY_MAX = 3f
    const val TRACKPAD_SENSITIVITY_DEFAULT = 1f

    const val THEME_SYSTEM = "system"
    const val THEME_LIGHT = "light"
    const val THEME_DARK = "dark"

    private var version by mutableIntStateOf(0)

    /** Read inside a @Composable to subscribe to any pref change. */
    @Composable
    @ReadOnlyComposable
    fun observedVersion(): Int = version

    private fun bump() {
        version++
    }

    private fun prefs(ctx: Context) =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun themeMode(ctx: Context): String =
        prefs(ctx).getString(KEY_THEME_MODE, THEME_SYSTEM) ?: THEME_SYSTEM

    fun setThemeMode(ctx: Context, mode: String) {
        prefs(ctx).edit { putString(KEY_THEME_MODE, mode) }
        bump()
    }

    /** Material You wallpaper colors on Android 12+; hand palette below that. */
    fun dynamicColor(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_DYNAMIC_COLOR, true)

    fun setDynamicColor(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_DYNAMIC_COLOR, enabled) }
        bump()
    }

    fun keepScreenOn(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_KEEP_SCREEN_ON, true)

    fun setKeepScreenOn(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_KEEP_SCREEN_ON, enabled) }
        bump()
    }

    /** T2: Home reminds when Android may stop a background feature. */
    fun backgroundReminder(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_BACKGROUND_REMINDER, true)

    fun setBackgroundReminder(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_BACKGROUND_REMINDER, enabled) }
        bump()
    }

    /** R2: a session whose link went away reconnects by itself. */
    fun autoReconnect(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_AUTO_RECONNECT, true)

    fun setAutoReconnect(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_AUTO_RECONNECT, enabled) }
        bump()
    }

    /** Show the PIN digits in the pairing sheet without tapping the eye. */
    fun revealPin(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_REVEAL_PIN, false)

    fun setRevealPin(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_REVEAL_PIN, enabled) }
        bump()
    }

    fun haptics(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_HAPTICS, true)

    /** Wake opens the session once the computer answers (N6). */
    fun connectAfterWake(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CONNECT_AFTER_WAKE, true)

    fun setConnectAfterWake(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CONNECT_AFTER_WAKE, enabled) }
        bump()
    }

    /** The session says when another phone drives the computer (N7). */
    fun showDriving(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_SHOW_DRIVING, true)

    fun setShowDriving(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_SHOW_DRIVING, enabled) }
        bump()
    }

    /** Connect checks first, and offers "Wake and connect" to a computer that does not answer (N6). */
    fun offerWake(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_OFFER_WAKE, true)

    fun setOfferWake(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_OFFER_WAKE, enabled) }
        bump()
    }

    fun setHaptics(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_HAPTICS, enabled) }
        bump()
    }

    /** Whether the desktop's sound plays on the phone during a session. */
    fun desktopSound(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_DESKTOP_SOUND, true)

    fun setDesktopSound(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_DESKTOP_SOUND, enabled) }
        bump()
    }

    /** How far the cursor moves per finger pixel on the trackpad. */
    fun trackpadSensitivity(ctx: Context): Float =
        prefs(ctx).getFloat(KEY_TRACKPAD_SENSITIVITY, TRACKPAD_SENSITIVITY_DEFAULT)
            .coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX)

    fun setTrackpadSensitivity(ctx: Context, value: Float) {
        prefs(ctx).edit {
            putFloat(KEY_TRACKPAD_SENSITIVITY, value.coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX))
        }
        bump()
    }
}
