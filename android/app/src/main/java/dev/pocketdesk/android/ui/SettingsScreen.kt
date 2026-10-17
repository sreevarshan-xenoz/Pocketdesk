package dev.pocketdesk.android.ui

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.compat.Android17
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.doctor.BackgroundLimit
import dev.pocketdesk.android.doctor.BackgroundRunning
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings, in groups (U4): This phone, Session, Files and links, In the
 * background, Notifications and texts, Automation, Pairing, Wake,
 * Appearance, System, About.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    Prefs.observedVersion()

    // A background feature runs from the notification listener, which keeps
    // the link while the app is away: turning one on asks for its access.
    fun askForListener(on: Boolean) {
        if (on && !PhoneNotificationPrefs.accessGranted(context)) {
            runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        LlIcon(LlIcons.ArrowBack, stringResource(R.string.settings_back))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier =
                Modifier
                    .fillMaxSize()
                    .padding(padding)
                    .verticalScroll(rememberScrollState())
                    .padding(bottom = 24.dp),
        ) {
            // U3: what this phone calls itself to computers.
            SettingsHeader(stringResource(R.string.settings_this_phone))
            var phoneName by remember { mutableStateOf(Prefs.phoneName(context)) }
            // Applied as Settings closes, not per key: a changed name closes
            // the control link so the next one carries it.
            DisposableEffect(Unit) {
                onDispose { Thread { dev.pocketdesk.android.PhoneName.apply(context) }.start() }
            }
            OutlinedTextField(
                value = phoneName,
                onValueChange = {
                    phoneName = it.take(dev.pocketdesk.android.ComputerName.MAX_CHARS * 2)
                    Prefs.setPhoneName(context, phoneName)
                },
                label = { Text(stringResource(R.string.settings_phone_name)) },
                placeholder = { Text(dev.pocketdesk.android.PhoneName.deviceName(context)) },
                supportingText = { Text(stringResource(R.string.settings_phone_name_summary)) },
                singleLine = true,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 4.dp),
            )

            SettingsHeader(stringResource(R.string.settings_session))
            SettingSwitch(
                stringResource(R.string.settings_keep_screen_on),
                stringResource(R.string.settings_keep_screen_on_summary),
                Prefs.keepScreenOn(context),
                { Prefs.setKeepScreenOn(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_auto_reconnect),
                stringResource(R.string.settings_auto_reconnect_summary),
                Prefs.autoReconnect(context),
                { Prefs.setAutoReconnect(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_capture_mouse),
                // A touchpad too on Android 17 (N4).
                stringResource(
                    if (Android17.captures(android.os.Build.VERSION.SDK_INT, false, false, true)) {
                        R.string.settings_capture_mouse_summary_17
                    } else {
                        R.string.settings_capture_mouse_summary
                    },
                ),
                Prefs.captureMouse(context),
                { Prefs.setCaptureMouse(context, it) },
            )
            TrackpadSpeed()
            SettingSwitch(
                stringResource(R.string.settings_show_driving),
                stringResource(R.string.settings_show_driving_summary),
                Prefs.showDriving(context),
                { Prefs.setShowDriving(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_haptics),
                stringResource(R.string.settings_haptics_summary),
                Prefs.haptics(context),
                { Prefs.setHaptics(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_connection_details),
                stringResource(R.string.settings_connection_details_summary),
                Prefs.connectionDetails(context),
                { Prefs.setConnectionDetails(context, it) },
            )

            SettingsHeader(stringResource(R.string.settings_files_links))
            SettingSwitch(
                stringResource(R.string.settings_retry_transfers),
                stringResource(R.string.settings_retry_transfers_summary),
                Prefs.retryTransfers(context),
                { Prefs.setRetryTransfers(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_internet_files),
                stringResource(R.string.settings_internet_files_summary),
                Prefs.internetFiles(context),
                {
                    Prefs.setInternetFiles(context, it)
                    RustCore.setInternetFiles(it)
                },
            )
            SettingSwitch(
                stringResource(R.string.settings_links_from_computer),
                stringResource(R.string.settings_links_from_computer_summary),
                Prefs.linksFromComputer(context),
                { Prefs.setLinksFromComputer(context, it) },
            )
            PhoneFoldersSection()

            SettingsHeader(stringResource(R.string.settings_background))
            SettingSwitch(
                stringResource(R.string.settings_clipboard_background),
                stringResource(R.string.settings_clipboard_background_summary),
                Prefs.clipboardInBackground(context),
                {
                    Prefs.setClipboardInBackground(context, it)
                    askForListener(it)
                },
            )
            SettingSwitch(
                stringResource(R.string.settings_files_background),
                stringResource(R.string.settings_files_background_summary),
                Prefs.filesInBackground(context),
                {
                    Prefs.setFilesInBackground(context, it)
                    askForListener(it)
                },
            )
            // N1: the computer may ring this phone to find it.
            SettingSwitch(
                stringResource(R.string.settings_ring_from_computer),
                // Android 17 rings at the owner's alarm volume (N4).
                stringResource(
                    if (Android17.ringRaisesVolume(android.os.Build.VERSION.SDK_INT)) {
                        R.string.settings_ring_from_computer_summary
                    } else {
                        R.string.settings_ring_from_computer_summary_17
                    },
                ),
                Prefs.ringFromComputer(context),
                {
                    Prefs.setRingFromComputer(context, it)
                    askForListener(it)
                },
            )
            SettingSwitch(
                stringResource(R.string.settings_share_battery),
                stringResource(R.string.settings_share_battery_summary),
                Prefs.shareBattery(context),
                { Prefs.setShareBattery(context, it) },
            )
            // Android shows other apps' media only to an app with notification access.
            SettingSwitch(
                stringResource(R.string.settings_phone_media),
                stringResource(R.string.settings_phone_media_summary),
                Prefs.phoneMediaOnComputer(context),
                {
                    Prefs.setPhoneMediaOnComputer(context, it)
                    askForListener(it)
                },
            )
            ScreenshotsSection()
            // T2: whether Android holds those back while the phone sleeps.
            BackgroundSection()
            SettingSwitch(
                stringResource(R.string.settings_background_remind),
                stringResource(R.string.settings_background_remind_summary),
                Prefs.backgroundReminder(context),
                { Prefs.setBackgroundReminder(context, it) },
            )

            SettingsHeader(stringResource(R.string.settings_notifications_texts))
            PhoneNotificationsSection()
            TextsSection()

            SettingsHeader(stringResource(R.string.settings_automation))
            SettingSwitch(
                stringResource(R.string.settings_others_choose),
                stringResource(R.string.settings_others_choose_summary),
                Prefs.othersChooseComputer(context),
                { Prefs.setOthersChooseComputer(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_tell_other_apps),
                stringResource(R.string.settings_tell_other_apps_summary),
                Prefs.tellOtherApps(context),
                { Prefs.setTellOtherApps(context, it) },
            )

            SettingsHeader(stringResource(R.string.settings_pairing))
            SettingSwitch(
                stringResource(R.string.settings_reveal_pin),
                stringResource(R.string.settings_reveal_pin_summary),
                Prefs.revealPin(context),
                { Prefs.setRevealPin(context, it) },
            )

            // N6: what Wake does once the packet is out. How each computer
            // is woken is set on its card ("Wake settings").
            SettingsHeader(stringResource(R.string.settings_wake))
            SettingSwitch(
                stringResource(R.string.settings_connect_after_wake),
                stringResource(R.string.settings_connect_after_wake_summary),
                Prefs.connectAfterWake(context),
                { Prefs.setConnectAfterWake(context, it) },
            )
            SettingSwitch(
                stringResource(R.string.settings_offer_wake),
                stringResource(R.string.settings_offer_wake_summary),
                Prefs.offerWake(context),
                { Prefs.setOfferWake(context, it) },
            )

            SettingsHeader(stringResource(R.string.settings_appearance))
            Column(Modifier.selectableGroup()) {
                listOf(
                    Prefs.THEME_SYSTEM to stringResource(R.string.theme_system),
                    Prefs.THEME_LIGHT to stringResource(R.string.theme_light),
                    Prefs.THEME_DARK to stringResource(R.string.theme_dark),
                ).forEach { (value, label) ->
                    val selected = Prefs.themeMode(context) == value
                    ListItem(
                        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
                        leadingContent = { RadioButton(selected = selected, onClick = null) },
                        modifier = Modifier.selectable(selected = selected, role = Role.RadioButton) {
                            Prefs.setThemeMode(context, value)
                        },
                    )
                }
            }
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
                SettingSwitch(
                    stringResource(R.string.settings_dynamic),
                    stringResource(R.string.settings_dynamic_summary),
                    Prefs.dynamicColor(context),
                    { Prefs.setDynamicColor(context, it) },
                )
            }

            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
                SettingsHeader(stringResource(R.string.settings_system))
                SettingLink(stringResource(R.string.language), null) {
                    runCatching {
                        context.startActivity(
                            Intent(Settings.ACTION_APP_LOCALE_SETTINGS, ("package:${context.packageName}").toUri()),
                        )
                    }
                }
            }

            SettingsHeader(stringResource(R.string.settings_about))
            ListItem(
                headlineContent = {
                    Text(
                        stringResource(R.string.app_version, appVersionName(context)),
                        style = MaterialTheme.typography.bodyLarge,
                    )
                },
                supportingContent = {
                    Text(
                        stringResource(R.string.rust_core_version, RustCore.version),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                },
            )
        }
    }
}

/** F3: trackpad speed, saved when the thumb is let go; the slider carries the row's name. */
@Composable
private fun TrackpadSpeed() {
    val context = LocalContext.current
    val title = stringResource(R.string.settings_trackpad_speed)
    var speed by remember { mutableFloatStateOf(Prefs.trackpadSensitivity(context)) }
    ListItem(
        headlineContent = { Text(title, style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            Column {
                Text(
                    stringResource(R.string.settings_trackpad_speed_summary),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Slider(
                    value = speed,
                    onValueChange = { speed = it },
                    onValueChangeFinished = { Prefs.setTrackpadSensitivity(context, speed) },
                    valueRange = Prefs.TRACKPAD_SENSITIVITY_MIN..Prefs.TRACKPAD_SENSITIVITY_MAX,
                    // 0.25 apart.
                    steps = 9,
                    modifier = Modifier.semantics { contentDescription = title },
                )
            }
        },
        trailingContent = { Text("%.2f×".format(speed), style = MaterialTheme.typography.labelLarge) },
    )
}

private fun appVersionName(context: android.content.Context): String =
    runCatching {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName
    }.getOrNull().orEmpty()

/**
 * T2: the battery status, read again each time the app comes back (from the
 * battery page, say), and the maker's own guide, which matters whatever
 * Android's own setting says.
 */
@Composable
private fun BackgroundSection() {
    val context = LocalContext.current
    val lifecycleOwner = androidx.compose.ui.platform.LocalLifecycleOwner.current
    var version by remember { mutableIntStateOf(0) }
    DisposableEffect(lifecycleOwner) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_RESUME) version++
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    val limit = remember(version) { BackgroundRunning.limit(context) }
    val guide = remember { BackgroundRunning.makerGuide(android.os.Build.MANUFACTURER) }
    SettingLink(
        stringResource(R.string.settings_background_status),
        stringResource(
            when (limit) {
                BackgroundLimit.NONE -> R.string.bg_allowed
                BackgroundLimit.OPTIMIZED -> R.string.bg_optimized
                BackgroundLimit.RESTRICTED -> R.string.bg_restricted
            },
        ),
    ) { BackgroundRunning.openSettings(context, limit) }
    if (guide != null) {
        SettingLink(stringResource(R.string.bg_maker_guide), stringResource(R.string.bg_maker_guide_summary)) {
            if (!BackgroundRunning.openGuide(context, guide)) {
                android.widget.Toast.makeText(context, guide, android.widget.Toast.LENGTH_LONG).show()
            }
        }
    }
}

/** Under a switch whose feature needs notification access it does not have: the way to it. */
@Composable
fun AllowNotificationAccess() {
    val context = LocalContext.current
    SettingLink(stringResource(R.string.settings_allow_access), null) {
        runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
    }
}
