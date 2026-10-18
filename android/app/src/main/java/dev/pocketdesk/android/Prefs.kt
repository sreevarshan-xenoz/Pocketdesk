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
    private const val KEY_RETRY_TRANSFERS = "retry_transfers"
    private const val KEY_DOWNLOAD_TREE = "download_tree"
    private const val KEY_CONTROL_FROM_COMPUTER = "control_from_computer"
    private const val KEY_INTERNET_FILES = "internet_files"
    private const val KEY_OTHERS_CHOOSE_COMPUTER = "others_choose_computer"
    private const val KEY_TELL_OTHER_APPS = "tell_other_apps"
    private const val KEY_LINKS_FROM_COMPUTER = "links_from_computer"
    private const val KEY_SHARE_BATTERY = "share_battery"
    private const val KEY_PHONE_MEDIA = "phone_media_on_computer"
    private const val KEY_CLIPBOARD_BACKGROUND = "clipboard_in_background"
    private const val KEY_ANSWER_NOTIFICATIONS = "answer_notifications"
    private const val KEY_FILES_BACKGROUND = "files_in_background"
    private const val KEY_RING_FROM_COMPUTER = "ring_from_computer"
    private const val KEY_CAPTURE_MOUSE = "capture_mouse"
    private const val KEY_BACKGROUND_REMINDER = "background_reminder"
    private const val KEY_REVEAL_PIN = "reveal_pin"
    private const val KEY_HAPTICS = "haptics"
    private const val KEY_CONNECTION_DETAILS = "connection_details"
    private const val KEY_CONNECT_AFTER_WAKE = "connect_after_wake"
    private const val KEY_OFFER_WAKE = "offer_wake"
    private const val KEY_PHONE_NAME = "phone_name"
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

    /** T6: a file that fails to send is sent again, continuing where it stopped. */
    /** Whether other apps may name the computer a share goes to (T7b). Off by default. */
    fun othersChooseComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_OTHERS_CHOOSE_COMPUTER, false)

    fun setOthersChooseComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_OTHERS_CHOOSE_COMPUTER, enabled) }
        bump()
    }

    /**
     * Whether sessions, files, links and tasks are told to other apps as
     * broadcasts (ecosystem 1f). Off by default: any app may receive them.
     */
    fun tellOtherApps(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_TELL_OTHER_APPS, false)

    fun setTellOtherApps(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_TELL_OTHER_APPS, enabled) }
        bump()
    }

    /** Whether a link the computer hands over is shown as a notification (T8a). On by default. */
    fun linksFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_LINKS_FROM_COMPUTER, true)

    fun setLinksFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_LINKS_FROM_COMPUTER, enabled) }
        bump()
    }

    /**
     * Whether the phone tells the computer its battery (T8b): in a session,
     * and outside one over a link a background feature keeps. On by default.
     */
    fun shareBattery(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_SHARE_BATTERY, true)

    fun setShareBattery(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_SHARE_BATTERY, enabled) }
        bump()
    }

    /** Whether a session tells the computer what this phone plays and takes its presses (T8d). Off by default. */
    fun phoneMediaOnComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_PHONE_MEDIA, false)

    fun setPhoneMediaOnComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_PHONE_MEDIA, enabled) }
        bump()
    }

    /**
     * Whether the computer's clipboard reaches the phone without a session
     * (T10b): the notification listener keeps a link for it. Off by default.
     */
    fun clipboardInBackground(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CLIPBOARD_BACKGROUND, false)

    fun setClipboardInBackground(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CLIPBOARD_BACKGROUND, enabled) }
        bump()
    }

    /**
     * Whether the computer may press this phone's notifications' buttons and
     * reply to them (T10c). Off by default: an answer goes out as the owner.
     */
    fun answerNotifications(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_ANSWER_NOTIFICATIONS, false)

    fun setAnswerNotifications(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_ANSWER_NOTIFICATIONS, enabled) }
        bump()
    }

    /** Whether the computer may ring this phone to find it (N1); keeps the background link. Off by default. */
    fun ringFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_RING_FROM_COMPUTER, false)

    fun setRingFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_RING_FROM_COMPUTER, enabled) }
        bump()
    }

    /**
     * Whether files the computer sends arrive without a session (T10d): the
     * notification listener keeps a link for them. Off by default.
     */
    fun filesInBackground(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_FILES_BACKGROUND, false)

    fun setFilesInBackground(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_FILES_BACKGROUND, enabled) }
        bump()
    }

    /** Whether a mouse connected to the phone drives the computer's cursor in a session (T9c). On by default. */
    fun captureMouse(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CAPTURE_MOUSE, true)

    fun setCaptureMouse(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CAPTURE_MOUSE, enabled) }
        bump()
    }

    /** C8: the computer this phone casts to may control it (off by default). */
    fun controlFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CONTROL_FROM_COMPUTER, false)

    fun setControlFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CONTROL_FROM_COMPUTER, enabled) }
        bump()
    }

    /** C2: the folder files from computers go to (a tree uri), or null for Downloads/Pocketdesk. */
    fun downloadTree(ctx: Context): String? =
        prefs(ctx).getString(KEY_DOWNLOAD_TREE, null)?.takeIf { it.isNotEmpty() }

    fun setDownloadTree(ctx: Context, tree: String?) {
        prefs(ctx).edit { if (tree == null) remove(KEY_DOWNLOAD_TREE) else putString(KEY_DOWNLOAD_TREE, tree) }
        bump()
    }

    fun retryTransfers(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_RETRY_TRANSFERS, true)

    fun setRetryTransfers(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_RETRY_TRANSFERS, enabled) }
        bump()
    }

    /**
     * Whether files move with a computer this phone reaches by the internet
     * road, on that link (2d). On by default, as on the computer.
     */
    fun internetFiles(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_INTERNET_FILES, true)

    fun setInternetFiles(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_INTERNET_FILES, enabled) }
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

    /** The name this phone gives computers (U3); blank: Android's device name. */
    fun phoneName(ctx: Context): String =
        prefs(ctx).getString(KEY_PHONE_NAME, null).orEmpty()

    fun setPhoneName(ctx: Context, name: String) {
        prefs(ctx).edit { putString(KEY_PHONE_NAME, name) }
        bump()
    }

    private const val KEY_SHARE_TOKEN = "share_shortcut_token"

    /**
     * This install's secret in its Direct Share shortcuts' ids (N2): made
     * once, 16 hex digits, read only here, by the system and the launcher.
     */
    fun shareToken(ctx: Context): String {
        prefs(ctx).getString(KEY_SHARE_TOKEN, null)?.let { return it }
        val bytes = ByteArray(8).also { java.security.SecureRandom().nextBytes(it) }
        val token = bytes.joinToString("") { "%02x".format(it) }
        prefs(ctx).edit { putString(KEY_SHARE_TOKEN, token) }
        return token
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

    /** The session shows its numbers, not a dot for the connection (U4). */
    fun connectionDetails(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CONNECTION_DETAILS, false)

    fun setConnectionDetails(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CONNECTION_DETAILS, enabled) }
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
