package dev.pocketdesk.android.ui

import android.content.Intent
import android.provider.Settings
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.control.ControlRunner
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * C8: whether the computer this phone casts to may control it. On needs
 * Android's Accessibility service too, which only the owner can turn on in
 * Android's own settings; the row says when that is missing and opens them.
 */
@Composable
fun ControlSetting() {
    val context = LocalContext.current
    val resumes = rememberResumes()
    var on by remember { mutableStateOf(Prefs.controlFromComputer(context)) }
    // Read again on return: the owner may have turned the service on there.
    val serviceOn = remember(resumes, on) { ControlRunner.serviceEnabled(context) }
    val openAccessibility = {
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    SettingSwitch(
        title = stringResource(R.string.settings_control_from_computer),
        summary = stringResource(
            when {
                !on -> R.string.settings_control_summary_off
                !serviceOn -> R.string.settings_control_summary_needs_service
                else -> R.string.settings_control_summary_on
            },
        ),
        checked = on,
        warning = on && !serviceOn,
        onChange = { want ->
            on = want
            Prefs.setControlFromComputer(context, want)
            if (want && !ControlRunner.serviceEnabled(context)) openAccessibility()
        },
    )
    if (on && !serviceOn) {
        SettingLink(stringResource(R.string.settings_control_open_accessibility), null) { openAccessibility() }
    }
}

/**
 * M4: typing from the computer's keyboard. Its own switch, needing C8's
 * Accessibility service and the notification listener, which waits for
 * what the computer types.
 */
@Composable
fun TypingSetting() {
    val context = LocalContext.current
    val resumes = rememberResumes()
    var on by remember { mutableStateOf(Prefs.typingFromComputer(context)) }
    val serviceOn = remember(resumes, on) { ControlRunner.serviceEnabled(context) }
    val listening = remember(resumes, on) { PhoneNotificationPrefs.accessGranted(context) }
    val openAccessibility = {
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    SettingSwitch(
        title = stringResource(R.string.settings_typing_from_computer),
        summary = stringResource(
            when {
                !on -> R.string.settings_typing_summary_off
                !serviceOn -> R.string.settings_control_summary_needs_service
                !listening -> R.string.settings_typing_summary_needs_access
                else -> R.string.settings_typing_summary_on
            },
        ),
        checked = on,
        warning = on && (!serviceOn || !listening),
        onChange = { want ->
            on = want
            Prefs.setTypingFromComputer(context, want)
            if (want && !ControlRunner.serviceEnabled(context)) {
                openAccessibility()
            } else if (want && !PhoneNotificationPrefs.accessGranted(context)) {
                runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
            }
        },
    )
    if (on && !serviceOn) {
        SettingLink(stringResource(R.string.settings_control_open_accessibility), null) { openAccessibility() }
    } else if (on && !listening) {
        AllowNotificationAccess()
    }
}

/**
 * O2: screenshots and locks from the phone's own computer. Its own switch,
 * needing C8's Accessibility service, which does them, and the notification
 * listener, which waits for the asks.
 */
@Composable
fun PhoneActionsSetting() {
    val context = LocalContext.current
    // Android 9 and later only (Q3 review: below it every ask was refused).
    if (android.os.Build.VERSION.SDK_INT < dev.pocketdesk.android.control.PhoneActionRule.MIN_SDK) return
    val resumes = rememberResumes()
    var on by remember { mutableStateOf(Prefs.phoneActionsFromComputer(context)) }
    val serviceOn = remember(resumes, on) { ControlRunner.serviceEnabled(context) }
    val listening = remember(resumes, on) { PhoneNotificationPrefs.accessGranted(context) }
    // What carries a screenshot here (Q3 review: on, it went nowhere unsaid).
    val carried = remember(resumes, on) {
        dev.pocketdesk.android.files.ScreenshotPrefs.on(context) &&
            dev.pocketdesk.android.files.ScreenshotPrefs.granted(context) &&
            (dev.pocketdesk.android.files.ScreenshotPrefs.clipboard(context) ||
                dev.pocketdesk.android.files.ScreenshotPrefs.save(context))
    }
    val openAccessibility = {
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    SettingSwitch(
        title = stringResource(R.string.settings_phone_actions),
        summary = stringResource(
            when {
                !on -> R.string.settings_phone_actions_summary_off
                !serviceOn -> R.string.settings_control_summary_needs_service
                !listening -> R.string.settings_phone_actions_summary_needs_access
                !carried -> R.string.settings_phone_actions_summary_needs_screenshots
                else -> R.string.settings_phone_actions_summary_on
            },
        ),
        checked = on,
        warning = on && (!serviceOn || !listening || !carried),
        onChange = { want ->
            on = want
            Prefs.setPhoneActionsFromComputer(context, want)
            if (want && !ControlRunner.serviceEnabled(context)) {
                openAccessibility()
            } else if (want && !PhoneNotificationPrefs.accessGranted(context)) {
                runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
            }
        },
    )
    if (on && !serviceOn) {
        SettingLink(stringResource(R.string.settings_control_open_accessibility), null) { openAccessibility() }
    } else if (on && !listening) {
        AllowNotificationAccess()
    }
}

/**
 * M10: copies made on the phone sent by themselves. Its own switch, needing
 * C8's Accessibility service, which then watches for Android's clipboard
 * preview; Android 13 and later only, where the preview is shown.
 */
@Composable
fun CopiesSetting() {
    val context = LocalContext.current
    if (!dev.pocketdesk.android.clip.CopyWatchRule.supported(android.os.Build.VERSION.SDK_INT)) return
    val resumes = rememberResumes()
    var on by remember { mutableStateOf(Prefs.copiesByThemselves(context)) }
    val serviceOn = remember(resumes, on) { ControlRunner.serviceEnabled(context) }
    val openAccessibility = {
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    SettingSwitch(
        title = stringResource(R.string.settings_copies),
        summary = stringResource(
            when {
                !on -> R.string.settings_copies_summary_off
                !serviceOn -> R.string.settings_control_summary_needs_service
                else -> R.string.settings_copies_summary_on
            },
        ),
        checked = on,
        warning = on && !serviceOn,
        onChange = { want ->
            on = want
            Prefs.setCopiesByThemselves(context, want)
            dev.pocketdesk.android.control.ControlService.instance?.syncCopyWatch(want)
            if (want && !ControlRunner.serviceEnabled(context)) openAccessibility()
        },
    )
    if (on && !serviceOn) {
        SettingLink(stringResource(R.string.settings_control_open_accessibility), null) { openAccessibility() }
    }
}

/**
 * P2: the phone's own computer may list this phone's apps (their names
 * only) and open one, the notification listener waiting for the asks. It
 * opens at once with C8's Accessibility service, else as "Tap to open", so
 * the service is not asked for (Q3 review).
 */
@Composable
fun AppsSetting() {
    val context = LocalContext.current
    val resumes = rememberResumes()
    var on by remember { mutableStateOf(Prefs.appsFromComputer(context)) }
    val listening = remember(resumes, on) { PhoneNotificationPrefs.accessGranted(context) }
    SettingSwitch(
        title = stringResource(R.string.settings_phone_apps),
        summary = stringResource(
            when {
                !on -> R.string.settings_phone_apps_summary_off
                !listening -> R.string.settings_phone_actions_summary_needs_access
                else -> R.string.settings_phone_apps_summary_on
            },
        ),
        checked = on,
        warning = on && !listening,
        onChange = { want ->
            on = want
            Prefs.setAppsFromComputer(context, want)
            if (want && !PhoneNotificationPrefs.accessGranted(context)) {
                runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
            }
        },
    )
    if (on && !listening) {
        AllowNotificationAccess()
    }
}
