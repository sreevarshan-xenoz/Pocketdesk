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
    private const val KEY_VOLUME_KEYS = "volume_keys"
    private const val KEY_KEEP_FILE_DATES = "keep_file_dates"
    private const val KEY_DOWNLOAD_TREE = "download_tree"
    private const val KEY_CONTROL_FROM_COMPUTER = "control_from_computer"
    private const val KEY_PHONE_ACTIONS_FROM_COMPUTER = "phone_actions_from_computer"
    private const val KEY_INTERNET_FILES = "internet_files"
    private const val KEY_OTHERS_CHOOSE_COMPUTER = "others_choose_computer"
    private const val KEY_TELL_OTHER_APPS = "tell_other_apps"
    private const val KEY_OTHERS_RUN_TASKS = "others_run_tasks"
    private const val KEY_LINKS_FROM_COMPUTER = "links_from_computer"
    private const val KEY_COMPUTER_ALERTS = "computer_alerts"
    private const val KEY_PEEK = "peek_on_home"
    private const val KEY_OFFER_MY_COMPUTERS = "offer_my_computers"
    private const val KEY_INTRODUCE_PENDING = "introduce_pending"
    private const val KEY_INTRODUCE_LAST = "introduce_last"
    private const val KEY_QUESTIONS = "questions_from_computer"
    private const val KEY_SHARE_BATTERY = "share_battery"
    private const val KEY_STATE_WIDGET_AUTO = "state_widget_auto"
    private const val KEY_PHONE_MEDIA = "phone_media_on_computer"
    private const val KEY_PHONE_MEDIA_BACKGROUND = "phone_media_background"
    private const val KEY_COMPUTER_MEDIA_CONTROLS = "computer_media_controls"
    private const val KEY_CLIPBOARD_BACKGROUND = "clipboard_in_background"
    private const val KEY_CLIPBOARD_IMAGES_BACKGROUND = "clipboard_images_in_background"
    private const val KEY_RECEIVED_IMAGES_CLIPBOARD = "received_images_clipboard"
    private const val KEY_ANSWER_NOTIFICATIONS = "answer_notifications"
    private const val KEY_QUIET_IN_HAND = "quiet_in_hand"
    private const val KEY_TOAST_WHERE_WORKING = "toast_where_working"
    private const val KEY_FILES_BACKGROUND = "files_in_background"
    private const val KEY_RING_FROM_COMPUTER = "ring_from_computer"
    private const val KEY_CAPTURE_MOUSE = "capture_mouse"
    private const val KEY_BACKGROUND_REMINDER = "background_reminder"
    private const val KEY_HEALTH_ALERTS = "health_alerts"
    private const val KEY_STAY_LINKED = "stay_linked"
    private const val KEY_LAST_FREEZE = "last_freeze_ms"
    private const val KEY_LAST_FREEZE_LINKED = "last_freeze_linked"
    private const val KEY_HEALTH_DISMISSED = "health_dismissed"
    private const val KEY_HEALTH_SEEN = "health_seen"
    private const val KEY_LAST_FINGERPRINT = "last_fingerprint"
    private const val KEY_REVEAL_PIN = "reveal_pin"
    private const val KEY_HAPTICS = "haptics"
    private const val KEY_CONNECTION_DETAILS = "connection_details"
    private const val KEY_CONNECT_AFTER_WAKE = "connect_after_wake"
    private const val KEY_OFFER_WAKE = "offer_wake"
    private const val KEY_VERSION_HINTS = "version_hints"
    private const val KEY_PHONE_NAME = "phone_name"
    private const val KEY_SHOW_DRIVING = "show_driving"
    private const val KEY_DESKTOP_SOUND = "desktop_sound"
    private const val KEY_TRACKPAD_SENSITIVITY = "trackpad_sensitivity"
    private const val KEY_CALLS_ON = "calls_on"
    private const val KEY_CALLS_DECLINE = "calls_decline"
    private const val KEY_AIR_MOUSE_SPEED = "air_mouse_speed"
    private const val KEY_TALK_TIMER_MINUTES = "talk_timer_minutes"
    private const val KEY_TALK_TIMER_BUZZ = "talk_timer_buzz"
    private const val KEY_AIR_MOUSE = "air_mouse"
    private const val KEY_AIR_LASER = "air_laser"
    private const val KEY_PEN_CURVE = "pen_curve"
    private const val KEY_GAMEPAD_CONTROLLER = "gamepad_controller"
    private const val KEY_DECK_COLUMNS = "deck_columns"
    private const val KEY_CAMERA_ASKS = "camera_asks"
    private const val KEY_CAMERA_CLIPBOARD = "camera_clipboard"

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

    /** M1: a quiet foreground service keeps the background link through the screen off. On by default. */
    fun stayLinked(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_STAY_LINKED, true)

    fun setStayLinked(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit {
            putBoolean(KEY_STAY_LINKED, enabled)
            // A freeze seen with it off is the switch's story, not news once it is on.
            if (enabled) {
                remove(KEY_LAST_FREEZE)
                remove(KEY_LAST_FREEZE_LINKED)
            }
        }
        bump()
    }

    /** M1: when the listener last saw the app frozen (wall clock), or null. */
    fun lastFreezeMs(ctx: Context): Long? =
        prefs(ctx).getLong(KEY_LAST_FREEZE, 0L).takeIf { it > 0L }

    /** M1: whether the link's service was running when that freeze was seen. */
    fun lastFreezeLinked(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_LAST_FREEZE_LINKED, false)

    /** M1: record a freeze seen now, and whether the link's service ran through it. */
    fun setLastFreeze(ctx: Context, ms: Long, linked: Boolean) {
        prefs(ctx).edit {
            putLong(KEY_LAST_FREEZE, ms)
            putBoolean(KEY_LAST_FREEZE_LINKED, linked)
        }
        // Home shows it as it comes (M1 review).
        bump()
    }

    /** D1: Home and a notification after updates say what stopped working. On by default. */
    fun healthAlerts(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_HEALTH_ALERTS, true)

    fun setHealthAlerts(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_HEALTH_ALERTS, enabled) }
        bump()
    }

    /** D1: the set of problems the owner put off with "Not now" ([dev.pocketdesk.android.doctor.Health.key]). */
    fun healthDismissed(ctx: Context): String? = prefs(ctx).getString(KEY_HEALTH_DISMISSED, null)

    fun setHealthDismissed(ctx: Context, key: String?) {
        prefs(ctx).edit { putString(KEY_HEALTH_DISMISSED, key) }
        bump()
    }

    /**
     * D1: the problems Home last found ([dev.pocketdesk.android.doctor.Health.key]),
     * so an update tells only of new ones. No [bump]: Home writes it as it reads.
     */
    fun healthSeen(ctx: Context): String? = prefs(ctx).getString(KEY_HEALTH_SEEN, null)

    fun setHealthSeen(ctx: Context, key: String) {
        if (healthSeen(ctx) != key) prefs(ctx).edit { putString(KEY_HEALTH_SEEN, key) }
    }

    /** D1: the system build seen at the last boot, to tell a system update from a restart. */
    fun lastFingerprint(ctx: Context): String? = prefs(ctx).getString(KEY_LAST_FINGERPRINT, null)

    fun setLastFingerprint(ctx: Context, fingerprint: String) {
        prefs(ctx).edit { putString(KEY_LAST_FINGERPRINT, fingerprint) }
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

    /** O1: automation apps may start the computer's tasks and press its deck (off). */
    fun othersRunTasks(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_OTHERS_RUN_TASKS, false)

    fun setOthersRunTasks(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_OTHERS_RUN_TASKS, enabled) }
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

    /** L4: a number the computer asks to dial opens the dialer (off by default). */
    fun callsFromComputer(ctx: Context): Boolean = prefs(ctx).getBoolean("calls_from_computer", false)

    fun setCallsFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("calls_from_computer", enabled) }
        bump()
    }

    /** L6: commands from the computer go to the automation apps (off by default). */
    fun commandsFromComputer(ctx: Context): Boolean = prefs(ctx).getBoolean("commands_from_computer", false)

    fun setCommandsFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("commands_from_computer", enabled) }
        bump()
    }

    /** L6: the automation app commands go to, or null for the first known one installed. */
    fun commandsOnlyTo(ctx: Context): String? = prefs(ctx).getString("commands_only_to", null)

    fun setCommandsOnlyTo(ctx: Context, pkg: String?) {
        prefs(ctx).edit { if (pkg == null) remove("commands_only_to") else putString("commands_only_to", pkg) }
        bump()
    }

    /** Whether a link the computer hands over is shown as a notification (T8a). On by default. */
    fun linksFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_LINKS_FROM_COMPUTER, true)

    fun setLinksFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_LINKS_FROM_COMPUTER, enabled) }
        bump()
    }

    /** Alerts from `pocketdesk done` shown with no session open (BF1); on by default. */
    fun computerAlerts(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_COMPUTER_ALERTS, true)

    fun setComputerAlerts(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_COMPUTER_ALERTS, enabled) }
        bump()
    }

    /** Peek in each computer's menu on Home (BF2); on by default. */
    fun peek(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_PEEK, true)

    fun setPeek(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_PEEK, enabled) }
        bump()
    }

    /**
     * Ecosystem phase 4: after pairing with a computer by its own code, offer
     * the owner's other computers it is paired with both ways. On by default;
     * each still waits for a yes here.
     */
    fun offerMyComputers(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_OFFER_MY_COMPUTERS, true)

    fun setOfferMyComputers(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_OFFER_MY_COMPUTERS, enabled) }
        bump()
    }

    /** The computer just paired with, whose other computers Home offers next. */
    fun markIntroducePending(ctx: Context, address: String) {
        prefs(ctx).edit { putString(KEY_INTRODUCE_PENDING, address) }
    }

    /** The computer just paired with, taken (offered once). */
    fun takeIntroducePending(ctx: Context): String? {
        val address = prefs(ctx).getString(KEY_INTRODUCE_PENDING, null)
        if (address != null) prefs(ctx).edit { remove(KEY_INTRODUCE_PENDING) }
        return address?.takeIf { it.isNotBlank() }
    }

    /** How the last introduction went ([dev.pocketdesk.android.pair.IntroduceRule.encode]). */
    fun introduceLast(ctx: Context): String? = prefs(ctx).getString(KEY_INTRODUCE_LAST, null)

    fun setIntroduceLast(ctx: Context, kept: String) {
        prefs(ctx).edit { putString(KEY_INTRODUCE_LAST, kept) }
        bump()
    }

    /** Questions from `pocketdesk ask` offered with no session open (BF3); on by default. */
    fun questionsFromComputer(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_QUESTIONS, true)

    fun setQuestionsFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_QUESTIONS, enabled) }
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

    /**
     * Whether the state widget (G1) asks the computer at the system's
     * widget period (at most every 30 minutes). On by default; off, it asks
     * only when tapped or placed.
     */
    fun stateWidgetAuto(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_STATE_WIDGET_AUTO, true)

    fun setStateWidgetAuto(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_STATE_WIDGET_AUTO, enabled) }
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
     * Whether the listener tells the own computer what this phone plays
     * without a session, with its cover (N2). Off by default: titles leave
     * the phone in the background.
     */
    fun phoneMediaInBackground(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_PHONE_MEDIA_BACKGROUND, false)

    fun setPhoneMediaInBackground(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_PHONE_MEDIA_BACKGROUND, enabled) }
        bump()
    }

    /**
     * Whether the computer's music shows in this phone's media controls
     * (N5): the listener keeps the link and asks the computer to tell it.
     * Off by default: titles and covers come to the lock screen.
     */
    fun computerMediaInControls(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_COMPUTER_MEDIA_CONTROLS, false)

    fun setComputerMediaInControls(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_COMPUTER_MEDIA_CONTROLS, enabled) }
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
     * M3: with the computer's clipboard in the background, an image copied
     * there is fetched onto this phone's clipboard too, on Wi-Fi. Off by
     * default.
     */
    fun clipboardImagesInBackground(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CLIPBOARD_IMAGES_BACKGROUND, false)

    fun setClipboardImagesInBackground(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CLIPBOARD_IMAGES_BACKGROUND, enabled) }
        bump()
    }

    /**
     * O7: an image the computer sends this phone on its own also goes onto
     * its clipboard. Off by default: a photo sent to keep would replace what
     * was copied.
     */
    fun receivedImagesToClipboard(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_RECEIVED_IMAGES_CLIPBOARD, false)

    fun setReceivedImagesToClipboard(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_RECEIVED_IMAGES_CLIPBOARD, enabled) }
    }

    /** A notification's picture goes to the computer with it (K3). Off by default: a picture may be private. */
    fun notificationPictures(ctx: Context): Boolean =
        prefs(ctx).getBoolean("notification_pictures", false)

    fun setNotificationPictures(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("notification_pictures", enabled) }
        bump()
    }

    /** Whether a shared app's ongoing progress and timers go to the computer (N4). Off by default. */
    fun liveUpdates(ctx: Context): Boolean = prefs(ctx).getBoolean("live_updates", false)

    fun setLiveUpdates(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("live_updates", enabled) }
    }

    /** Whether the computer may open one of this phone's notifications here (N3). Off by default. */
    fun letComputerOpen(ctx: Context): Boolean = prefs(ctx).getBoolean("let_computer_open", false)

    fun setLetComputerOpen(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("let_computer_open", enabled) }
    }

    /** L1: a notification closed on the computer is cleared here (on by default). */
    fun letComputerClear(ctx: Context): Boolean = prefs(ctx).getBoolean("let_computer_clear", true)

    fun setLetComputerClear(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("let_computer_clear", enabled) }
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

    /** M6: no toast on the computer for a notification that came while the phone was in use. Off by default. */
    /** P5: a ringing alarm or finished timer rings on the computer too. Off. */
    fun alarmsOnComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean("alarms_on_computer", false)

    fun setAlarmsOnComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("alarms_on_computer", enabled) }
    }

    fun quietInHand(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_QUIET_IN_HAND, false)

    fun setQuietInHand(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_QUIET_IN_HAND, enabled) }
        bump()
    }

    /**
     * N7: a notification pops up only on the computer in use, while one is;
     * the others keep it on their Notifications page. Keeps a link to each
     * other computer picked. Off by default.
     */
    fun toastWhereWorking(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_TOAST_WHERE_WORKING, false)

    fun setToastWhereWorking(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_TOAST_WHERE_WORKING, enabled) }
        bump()
    }

    /** P11: a ring from the computer vibrates too. On. */
    fun ringVibrates(ctx: Context): Boolean = prefs(ctx).getBoolean("ring_vibrates", true)

    fun setRingVibrates(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("ring_vibrates", enabled) }
        bump()
    }

    /** P11: a ring from the computer blinks the torch too. On. */
    fun ringFlashes(ctx: Context): Boolean = prefs(ctx).getBoolean("ring_flashes", true)

    fun setRingFlashes(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("ring_flashes", enabled) }
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

    /** A game controller connected to the phone plays on the computer (E4). */
    fun gamepadController(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_GAMEPAD_CONTROLLER, true)

    fun setGamepadController(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_GAMEPAD_CONTROLLER, enabled) }
        bump()
    }

    /** Whether a mouse connected to the phone drives the computer's cursor in a session (T9c). On by default. */
    fun captureMouse(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CAPTURE_MOUSE, true)

    fun setCaptureMouse(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CAPTURE_MOUSE, enabled) }
        bump()
    }

    /** K5: "Show screen" also sends what the phone's apps play (off by default). */
    fun castSound(ctx: Context): Boolean = prefs(ctx).getBoolean("cast_sound", false)

    fun setCastSound(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("cast_sound", enabled) }
        bump()
    }

    /** O2: the phone's own computer may take screenshots and lock it (off by default). */
    fun phoneActionsFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_PHONE_ACTIONS_FROM_COMPUTER, false)

    fun setPhoneActionsFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_PHONE_ACTIONS_FROM_COMPUTER, enabled) }
        bump()
    }

    /** P2: the phone's own computer may list its apps and open one (off by default). */
    fun appsFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean("apps_from_computer", false)

    fun setAppsFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("apps_from_computer", enabled) }
        bump()
    }

    /** C8: the computer this phone casts to may control it (off by default). */
    fun controlFromComputer(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_CONTROL_FROM_COMPUTER, false)

    fun setControlFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CONTROL_FROM_COMPUTER, enabled) }
        bump()
    }

    /**
     * M4: what is typed on the phone's own computer's keyboard is typed here
     * (off by default). Needs C8's Accessibility service, never C8's switch:
     * typing needs no cast.
     */
    fun typingFromComputer(ctx: Context): Boolean = prefs(ctx).getBoolean("typing_from_computer", false)

    fun setTypingFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("typing_from_computer", enabled) }
        bump()
    }

    /**
     * BK5: the phone's own computer may carry its keyboard and mouse onto
     * this phone, used as another screen (off by default). Needs C8's
     * Accessibility service, never C8's switch.
     */
    fun keyboardMouseFromComputer(ctx: Context): Boolean = prefs(ctx).getBoolean("keyboard_mouse_from_computer", false)

    fun setKeyboardMouseFromComputer(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("keyboard_mouse_from_computer", enabled) }
        bump()
    }

    /**
     * M10: copies made on the phone go to its computer by themselves (off
     * by default). Needs C8's Accessibility service, never C8's switch.
     */
    fun copiesByThemselves(ctx: Context): Boolean = prefs(ctx).getBoolean("copies_by_themselves", false)

    fun setCopiesByThemselves(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean("copies_by_themselves", enabled) }
        bump()
    }

    /** C2: the folder files from computers go to (a tree uri), or null for Downloads/Pocketdesk. */
    fun downloadTree(ctx: Context): String? =
        prefs(ctx).getString(KEY_DOWNLOAD_TREE, null)?.takeIf { it.isNotEmpty() }

    fun setDownloadTree(ctx: Context, tree: String?) {
        prefs(ctx).edit { if (tree == null) remove(KEY_DOWNLOAD_TREE) else putString(KEY_DOWNLOAD_TREE, tree) }
        bump()
    }

    /** The phone's volume keys change the computer's volume in Remote and sound (K4). On by default. */
    fun volumeKeys(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_VOLUME_KEYS, true)

    fun setVolumeKeys(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_VOLUME_KEYS, enabled) }
        bump()
    }

    fun retryTransfers(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_RETRY_TRANSFERS, true)

    fun setRetryTransfers(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_RETRY_TRANSFERS, enabled) }
        bump()
    }

    /** Files keep the date they were last changed, both ways (K1). On by default. */
    fun keepFileDates(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_KEEP_FILE_DATES, true)

    fun setKeepFileDates(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_KEEP_FILE_DATES, enabled) }
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

    /** Item 4: say which to update when a computer runs another Pocketdesk release (on). */
    fun versionHints(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_VERSION_HINTS, true)

    fun setVersionHints(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_VERSION_HINTS, enabled) }
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
    /** How fast the air mouse moves the pointer (F4), as the trackpad's range. */
    fun airMouseSpeed(ctx: Context): Float =
        prefs(ctx).getFloat(KEY_AIR_MOUSE_SPEED, 1f).coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX)

    fun setAirMouseSpeed(ctx: Context, value: Float) {
        prefs(ctx).edit { putFloat(KEY_AIR_MOUSE_SPEED, value.coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX)) }
        bump()
    }

    /** The talk timer's last length in minutes (H2), 20 until one is chosen. */
    fun talkTimerMinutes(ctx: Context): Int = prefs(ctx).getInt(KEY_TALK_TIMER_MINUTES, 20)

    fun setTalkTimerMinutes(ctx: Context, minutes: Int) {
        prefs(ctx).edit { putInt(KEY_TALK_TIMER_MINUTES, minutes) }
    }

    /** Whether the talk timer buzzes at five minutes, one minute and the end (H2). On by default. */
    fun talkTimerBuzz(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_TALK_TIMER_BUZZ, true)

    fun setTalkTimerBuzz(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_TALK_TIMER_BUZZ, on) }
        bump()
    }

    /** The remote shows the air mouse rather than the trackpad (F4): the last choice. */
    fun airMouse(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_AIR_MOUSE, false)

    /** Whether the air mouse moves the computer's laser dot instead of its pointer (G4). */
    fun airLaser(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_AIR_LASER, false)

    fun setAirLaser(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_AIR_LASER, on) }
        bump()
    }

    fun setAirMouse(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_AIR_MOUSE, on) }
        bump()
    }

    /** Ringing calls are told to the computer (F5). */
    fun callsOn(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_CALLS_ON, false)

    fun setCallsOn(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CALLS_ON, on) }
        bump()
    }

    /** The computer may decline a ringing call (F5). */
    fun callsDecline(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_CALLS_DECLINE, false)

    fun setCallsDecline(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CALLS_DECLINE, on) }
        bump()
    }

    fun trackpadSensitivity(ctx: Context): Float =
        prefs(ctx).getFloat(KEY_TRACKPAD_SENSITIVITY, TRACKPAD_SENSITIVITY_DEFAULT)
            .coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX)

    fun setTrackpadSensitivity(ctx: Context, value: Float) {
        prefs(ctx).edit {
            putFloat(KEY_TRACKPAD_SENSITIVITY, value.coerceIn(TRACKPAD_SENSITIVITY_MIN, TRACKPAD_SENSITIVITY_MAX))
        }
        bump()
    }

    /**
     * The computer may ask for a photo, offered in a notification (F3). Off
     * by default, as the other features that keep a link to the computer.
     */
    fun cameraAsks(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_CAMERA_ASKS, false)

    fun setCameraAsks(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CAMERA_ASKS, on) }
        bump()
    }

    /** A photo asked for goes onto the computer's clipboard too (F3). */
    fun cameraClipboard(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_CAMERA_CLIPBOARD, true)

    fun setCameraClipboard(ctx: Context, on: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_CAMERA_CLIPBOARD, on) }
        bump()
    }

    /** How many of the computer's deck buttons a row holds (F1). */
    fun deckColumns(ctx: Context): Int =
        prefs(ctx).getInt(KEY_DECK_COLUMNS, dev.pocketdesk.android.ui.Deck.DEFAULT_COLUMNS)

    fun setDeckColumns(ctx: Context, columns: Int) {
        prefs(ctx).edit { putInt(KEY_DECK_COLUMNS, dev.pocketdesk.android.ui.Deck.columns(columns)) }
        bump()
    }

    /** How hard the pen must press for full ink (E3). */
    fun penCurve(ctx: Context): dev.pocketdesk.android.stream.PenCurve =
        dev.pocketdesk.android.stream.PenCurve.fromKey(prefs(ctx).getString(KEY_PEN_CURVE, null))

    fun setPenCurve(ctx: Context, curve: dev.pocketdesk.android.stream.PenCurve) {
        prefs(ctx).edit { putString(KEY_PEN_CURVE, curve.key) }
        bump()
    }
}
