package dev.pocketdesk.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.saveable.rememberSaveable
import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
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
import kotlinx.coroutines.launch

/**
 * Settings, in groups (U4): This phone, Session, Files and links, In the
 * background, Notifications and texts, Automation, Pairing, Wake,
 * Appearance, System, About.
 */
/** The longest search typed. */
private const val SEARCH_CHARS = 64

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

    var query by rememberSaveable { mutableStateOf("") }
    val filter = remember { SettingsFilter() }
    filter.query = query
    BackHandler(enabled = query.isNotEmpty()) { query = "" }
    val keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current

    val scroll = rememberScrollState()
    val jumps = remember { SettingsJumps() }

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
        // The search box and the groups' row stay on screen above the
        // scrolled settings (PR #575 review N4: in the scroll, the row was
        // above the fold after a first jump).
        Column(
            modifier =
                Modifier
                    .fillMaxSize()
                    .padding(padding),
        ) {
            // O8: find a setting; Back empties the box first.
            OutlinedTextField(
                value = query,
                onValueChange = { query = it.take(SEARCH_CHARS) },
                placeholder = { Text(stringResource(R.string.settings_find)) },
                leadingIcon = { LlIcon(LlIcons.Search, null) },
                trailingIcon = if (query.isNotEmpty()) {
                    {
                        IconButton(onClick = { query = "" }) {
                            LlIcon(LlIcons.Close, stringResource(R.string.settings_find_clear))
                        }
                    }
                } else {
                    null
                },
                singleLine = true,
                // Search closes the keyboard (O8 review).
                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                    imeAction = androidx.compose.ui.text.input.ImeAction.Search,
                ),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSearch = { keyboard?.hide() }),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 8.dp),
            )
            // UX1: the groups, to jump to one; not while searching, when the
            // search already shows only what matches.
            SettingsJumpRow(jumps, scroll, shown = !SettingsSearch.searching(query))
            Column(
                modifier = Modifier
                    .weight(1f)
                    .verticalScroll(scroll)
                    .padding(bottom = 24.dp),
            ) {
            if (filter.nothingFound()) {
                Text(
                    stringResource(R.string.settings_find_none, query.trim()),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    // Said to TalkBack as it comes (O8 review).
                    modifier = Modifier
                        .padding(horizontal = 16.dp, vertical = 12.dp)
                        .semantics { liveRegion = androidx.compose.ui.semantics.LiveRegionMode.Polite },
                )
            }
            CompositionLocalProvider(LocalSettingsFilter provides filter, LocalSettingsJumps provides jumps) {
            // U3: what this phone calls itself to computers.
            SettingsGroup(stringResource(R.string.settings_this_phone)) {
                var phoneName by remember { mutableStateOf(Prefs.phoneName(context)) }
                // Applied as Settings closes, not per key: a changed name closes
                // the control link so the next one carries it.
                DisposableEffect(Unit) {
                    onDispose { Thread { dev.pocketdesk.android.PhoneName.apply(context) }.start() }
                }
                SettingsBlock(
                    stringResource(R.string.settings_phone_name),
                    stringResource(R.string.settings_phone_name_summary),
                ) {
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
                }
                ControlSetting()
                TypingSetting()
                KeyboardMouseSetting()
                PhoneActionsSetting()
                AppsSetting()
                CopiesSetting()
                CastSoundSetting()
            }

            SettingsGroup(stringResource(R.string.settings_session)) {
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
                    stringResource(R.string.settings_volume_keys),
                    stringResource(R.string.settings_volume_keys_summary),
                    Prefs.volumeKeys(context),
                    { Prefs.setVolumeKeys(context, it) },
                )
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
            }

            // E6: what drives the computer, under a heading of its own.
            SettingsGroup(stringResource(R.string.settings_input)) {
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
                SettingSwitch(
                    stringResource(R.string.settings_smooth_scrolling),
                    stringResource(R.string.settings_smooth_scrolling_summary),
                    Prefs.smoothScrolling(context),
                    { Prefs.setSmoothScrolling(context, it) },
                )
                TrackpadSpeed()
                AirMouseSpeed()
                // H2: the talk timer in Remote.
                SettingSwitch(
                    stringResource(R.string.settings_talk_timer_buzz),
                    stringResource(R.string.settings_talk_timer_buzz_summary),
                    Prefs.talkTimerBuzz(context),
                    { Prefs.setTalkTimerBuzz(context, it) },
                )
                PenPressure()
                SettingSwitch(
                    stringResource(R.string.settings_gamepad_controller),
                    stringResource(R.string.settings_gamepad_controller_summary),
                    Prefs.gamepadController(context),
                    { Prefs.setGamepadController(context, it) },
                )
                DeckColumns()
            }

            SettingsGroup(stringResource(R.string.settings_files_links)) {
                // F3: a photo the computer asks for; the listener keeps its link.
                val cameraNeedsAccess = Prefs.cameraAsks(context) &&
                    !dev.pocketdesk.android.notify.PhoneNotificationPrefs.accessGranted(context)
                SettingSwitch(
                    stringResource(R.string.settings_camera_asks),
                    stringResource(
                        if (cameraNeedsAccess) R.string.settings_camera_asks_needs_access else R.string.settings_camera_asks_summary,
                    ),
                    Prefs.cameraAsks(context),
                    { Prefs.setCameraAsks(context, it) },
                    warning = cameraNeedsAccess,
                )
                if (cameraNeedsAccess) AllowNotificationAccess()
                UnderSwitch(Prefs.cameraAsks(context)) {
                    SettingSwitch(
                        stringResource(R.string.settings_camera_clipboard),
                        stringResource(R.string.settings_camera_clipboard_summary),
                        Prefs.cameraClipboard(context),
                        { Prefs.setCameraClipboard(context, it) },
                    )
                }
                SettingSwitch(
                    stringResource(R.string.settings_retry_transfers),
                    stringResource(R.string.settings_retry_transfers_summary),
                    Prefs.retryTransfers(context),
                    { Prefs.setRetryTransfers(context, it) },
                )
                SettingSwitch(
                    stringResource(R.string.settings_keep_file_dates),
                    stringResource(R.string.settings_keep_file_dates_summary),
                    Prefs.keepFileDates(context),
                    { Prefs.setKeepFileDates(context, it) },
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
                // BF4: "Note to computer" in the share sheet. Held here: the
                // share sheet's entry is no pref, so nothing else redraws the
                // switch (BF4 review).
                var noteShown by remember { mutableStateOf(dev.pocketdesk.android.NoteShare.shown(context)) }
                SettingSwitch(
                    stringResource(R.string.settings_note_to_computer),
                    stringResource(R.string.settings_note_to_computer_summary),
                    noteShown,
                    {
                        dev.pocketdesk.android.NoteShare.setShown(context, it)
                        noteShown = it
                    },
                )
                // BF1: `pocketdesk done` saying a command on the computer ended.
                SettingSwitch(
                    stringResource(R.string.settings_computer_alerts),
                    stringResource(R.string.settings_computer_alerts_summary),
                    Prefs.computerAlerts(context),
                    { Prefs.setComputerAlerts(context, it) },
                )
                // Ecosystem phase 4: after pairing, the owner's other computers.
                SettingSwitch(
                    stringResource(R.string.settings_offer_my_computers),
                    introduceSummary(context),
                    Prefs.offerMyComputers(context),
                    { Prefs.setOfferMyComputers(context, it) },
                )
                // BF2: a still of the computer's screen from Home.
                SettingSwitch(
                    stringResource(R.string.settings_peek),
                    stringResource(R.string.settings_peek_summary),
                    Prefs.peek(context),
                    { Prefs.setPeek(context, it) },
                )
                // BF3: `pocketdesk ask` waiting on this phone's Yes or No.
                SettingSwitch(
                    stringResource(R.string.settings_questions),
                    stringResource(R.string.settings_questions_summary),
                    Prefs.questionsFromComputer(context),
                    { Prefs.setQuestionsFromComputer(context, it) },
                )
                // L4: the dialer opens with a number the computer sends.
                SettingSwitch(
                    stringResource(R.string.settings_calls_from_computer),
                    stringResource(R.string.settings_calls_from_computer_summary),
                    Prefs.callsFromComputer(context),
                    { Prefs.setCallsFromComputer(context, it) },
                )
                // L6: the computer's commands to the automation apps.
                CommandsSetting()
                DownloadFolderSetting()
                // O7: an image the computer sends, in a session or not,
                // onto the clipboard as well, beside where files land
                // (Q3 review: it sat among the background rows).
                SettingSwitch(
                    stringResource(R.string.settings_received_images_clipboard),
                    stringResource(R.string.settings_received_images_clipboard_summary),
                    Prefs.receivedImagesToClipboard(context),
                    { Prefs.setReceivedImagesToClipboard(context, it) },
                )
                PhoneFoldersSection()
            }

            SettingsGroup(stringResource(R.string.settings_background)) {
                SettingSwitch(
                    stringResource(R.string.settings_clipboard_background),
                    stringResource(R.string.settings_clipboard_background_summary),
                    Prefs.clipboardInBackground(context),
                    {
                        Prefs.setClipboardInBackground(context, it)
                        askForListener(it)
                    },
                )
                // M3: an image copied on the computer too, on Wi-Fi.
                SettingSwitch(
                    stringResource(R.string.settings_clipboard_images_background),
                    stringResource(R.string.settings_clipboard_images_background_summary),
                    Prefs.clipboardImagesInBackground(context),
                    { Prefs.setClipboardImagesInBackground(context, it) },
                    enabled = Prefs.clipboardInBackground(context),
                    indent = true,
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
                // P11: it shakes and flashes the torch while it sounds.
                // Never greyed (Q3 review): a ring in a session reads them
                // too, whatever the switch above. Shown only where the
                // phone has the vibrator or the torch.
                if (hasVibrator(context)) {
                    SettingSwitch(
                        stringResource(R.string.settings_ring_vibrates),
                        stringResource(R.string.settings_ring_vibrates_summary),
                        Prefs.ringVibrates(context),
                        { Prefs.setRingVibrates(context, it) },
                        indent = true,
                    )
                }
                if (context.packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_FLASH)) {
                    SettingSwitch(
                        stringResource(R.string.settings_ring_flashes),
                        stringResource(R.string.settings_ring_flashes_summary),
                        Prefs.ringFlashes(context),
                        { Prefs.setRingFlashes(context, it) },
                        indent = true,
                    )
                }
                SettingSwitch(
                    stringResource(R.string.settings_share_battery),
                    stringResource(R.string.settings_share_battery_summary),
                    Prefs.shareBattery(context),
                    { Prefs.setShareBattery(context, it) },
                )
                // G1: the state widget's own refresh.
                SettingSwitch(
                    stringResource(R.string.settings_state_widget_auto),
                    stringResource(R.string.settings_state_widget_auto_summary),
                    Prefs.stateWidgetAuto(context),
                    { Prefs.setStateWidgetAuto(context, it) },
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
                // N2: the same without a session, from the listener.
                SettingSwitch(
                    stringResource(R.string.settings_phone_media_background),
                    stringResource(R.string.settings_phone_media_background_summary),
                    Prefs.phoneMediaInBackground(context),
                    {
                        Prefs.setPhoneMediaInBackground(context, it)
                        if (it) Prefs.setPhoneMediaOnComputer(context, true)
                        askForListener(it)
                    },
                )
                // N5: the other way, the computer's music in this phone's controls.
                SettingSwitch(
                    stringResource(R.string.settings_computer_media_controls),
                    stringResource(R.string.settings_computer_media_controls_summary),
                    Prefs.computerMediaInControls(context),
                    {
                        Prefs.setComputerMediaInControls(context, it)
                        askForListener(it)
                    },
                )
                ScreenshotsSection()
                CameraUploadsSection()
                RecentPhotosSection()
                // M5: a photo's place kept when it is sent.
                PhotoPlaceSection()
                // T2: whether Android holds those back while the phone sleeps.
                BackgroundSection()
                SettingSwitch(
                    stringResource(R.string.settings_background_remind),
                    stringResource(R.string.settings_background_remind_summary),
                    Prefs.backgroundReminder(context),
                    { Prefs.setBackgroundReminder(context, it) },
                )
            }

            SettingsGroup(stringResource(R.string.settings_notifications_texts)) {
                // E6: both Do Not Disturb settings in one place.
                // D2: the computer may turn on Do Not Disturb for a while. Its
                // own preferences, so the switch redraws from a local state.
                var letMute by remember { mutableStateOf(dev.pocketdesk.android.notify.QuietPrefs.letComputerMute(context)) }
                SettingSwitch(
                    stringResource(R.string.settings_let_computer_mute),
                    stringResource(R.string.settings_let_computer_mute_summary),
                    letMute,
                    {
                        dev.pocketdesk.android.notify.QuietPrefs.setLetComputerMute(context, it)
                        letMute = it
                        askForListener(it)
                    },
                )
                PhoneNotificationsSection()
                TextsSection()
                CallsSection()
            }

            SettingsGroup(stringResource(R.string.settings_automation)) {
                SettingSwitch(
                    stringResource(R.string.settings_others_choose),
                    stringResource(R.string.settings_others_choose_summary),
                    Prefs.othersChooseComputer(context),
                    { Prefs.setOthersChooseComputer(context, it) },
                )
                SettingSwitch(
                    stringResource(R.string.settings_others_run_tasks),
                    stringResource(R.string.settings_others_run_tasks_summary),
                    Prefs.othersRunTasks(context),
                    { Prefs.setOthersRunTasks(context, it) },
                )
                SettingSwitch(
                    stringResource(R.string.settings_tell_other_apps),
                    stringResource(R.string.settings_tell_other_apps_summary),
                    Prefs.tellOtherApps(context),
                    { Prefs.setTellOtherApps(context, it) },
                )
            }

            SettingsGroup(stringResource(R.string.settings_pairing)) {
                SettingSwitch(
                    stringResource(R.string.settings_reveal_pin),
                    stringResource(R.string.settings_reveal_pin_summary),
                    Prefs.revealPin(context),
                    { Prefs.setRevealPin(context, it) },
                )
                // Item 4: which to update when a computer runs another release.
                SettingSwitch(
                    stringResource(R.string.settings_version_hints),
                    stringResource(R.string.settings_version_hints_summary),
                    Prefs.versionHints(context),
                    { Prefs.setVersionHints(context, it) },
                )
            }

            // N6: what Wake does once the packet is out. How each computer
            // is woken is set on its card ("Wake settings").
            SettingsGroup(stringResource(R.string.settings_wake)) {
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
            }

            SettingsGroup(stringResource(R.string.settings_appearance)) {
                SettingsBlock(
                    stringResource(R.string.theme_system),
                    stringResource(R.string.theme_light),
                    stringResource(R.string.theme_dark),
                ) {
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
                }
                if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
                    SettingSwitch(
                        stringResource(R.string.settings_dynamic),
                        stringResource(R.string.settings_dynamic_summary),
                        Prefs.dynamicColor(context),
                        { Prefs.setDynamicColor(context, it) },
                    )
                }
            }

            SettingsGroup(stringResource(R.string.settings_system)) {
                // D1: what stopped working, on Home and after updates.
                SettingSwitch(
                    stringResource(R.string.settings_health),
                    stringResource(R.string.settings_health_summary),
                    Prefs.healthAlerts(context),
                    { Prefs.setHealthAlerts(context, it) },
                )
                // M1: a quiet foreground service keeps the background link with the screen off.
                SettingSwitch(
                    stringResource(R.string.settings_stay_linked),
                    stringResource(R.string.settings_stay_linked_summary),
                    Prefs.stayLinked(context),
                    {
                        Prefs.setStayLinked(context, it)
                        val app = context.applicationContext
                        Thread { runCatching { dev.pocketdesk.android.service.LinkKeeperService.sync(app) } }.start()
                    },
                )
                if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
                    SettingLink(stringResource(R.string.language), null) {
                        runCatching {
                            context.startActivity(
                                Intent(Settings.ACTION_APP_LOCALE_SETTINGS, ("package:${context.packageName}").toUri()),
                            )
                        }
                    }
                }
            }

            SettingsGroup(stringResource(R.string.settings_about)) {
                SettingsBlock(stringResource(R.string.app_version, appVersionName(context))) {
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
            }
        }
    }
}

/**
 * UX1: a row of Settings' groups that scrolls [scroll] to the one tapped.
 * Its own composable, so only it reads where the headings are (PR #575
 * review N5: read in the screen's lambda, each heading's place recomposed
 * every row); its own scroll is kept while a search hides it.
 */
@Composable
private fun SettingsJumpRow(jumps: SettingsJumps, scroll: androidx.compose.foundation.ScrollState, shown: Boolean) {
    val scope = rememberCoroutineScope()
    val rowScroll = rememberScrollState()
    if (!shown) return
    val groups = jumps.ordered()
    if (groups.isEmpty()) return
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rowScroll)
            .padding(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        groups.forEach { (label, top) ->
            // Said as "Go to Wake", not twelve bare names before the first
            // setting (PR #575 review N6).
            val goTo = stringResource(R.string.settings_jump_to, label)
            SuggestionChip(
                onClick = { scope.launch { scroll.animateScrollTo(top) } },
                label = { Text(label) },
                modifier = Modifier.semantics { contentDescription = goTo },
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
    if (!rememberSettingShown(title, stringResource(R.string.settings_trackpad_speed_summary))) return
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

/** F4: how fast the air mouse moves the pointer, on the trackpad's range. */
@Composable
private fun AirMouseSpeed() {
    val context = LocalContext.current
    val title = stringResource(R.string.settings_air_speed)
    var speed by remember { mutableFloatStateOf(Prefs.airMouseSpeed(context)) }
    if (!rememberSettingShown(title, stringResource(R.string.settings_air_speed_summary))) return
    ListItem(
        headlineContent = { Text(title, style = MaterialTheme.typography.bodyLarge) },
        supportingContent = {
            Column {
                Text(
                    stringResource(R.string.settings_air_speed_summary),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Slider(
                    value = speed,
                    onValueChange = { speed = it },
                    onValueChangeFinished = { Prefs.setAirMouseSpeed(context, speed) },
                    valueRange = Prefs.TRACKPAD_SENSITIVITY_MIN..Prefs.TRACKPAD_SENSITIVITY_MAX,
                    steps = 9,
                    modifier = Modifier.semantics { contentDescription = title },
                )
            }
        },
        trailingContent = { Text("%.2f×".format(speed), style = MaterialTheme.typography.labelLarge) },
    )
}

/** E3: how hard the pen presses for full ink, and what draws best. */
@Composable
private fun PenPressure() {
    val context = LocalContext.current
    val curves = dev.pocketdesk.android.stream.PenCurve.entries
    // Read again when a choice is saved, so the chosen one redraws (E6 review).
    Prefs.observedVersion()
    val chosen = Prefs.penCurve(context)
    // A row like its neighbours (E6): title and summary, the choice under,
    // which hides with it (O8 review).
    val title = stringResource(R.string.settings_pen_pressure)
    val summary = stringResource(R.string.settings_pen_pressure_summary)
    if (!rememberSettingShown(title, summary)) return
    SettingRow(title, summary)
    ChoiceRow(
        null,
        listOf(
            stringResource(R.string.pen_pressure_soft),
            stringResource(R.string.pen_pressure_even),
            stringResource(R.string.pen_pressure_firm),
        ),
        curves.indexOf(chosen),
        horizontal = 16.dp,
    ) { Prefs.setPenCurve(context, curves[it]) }
}

/** F1: how many of the computer's deck buttons a row holds. */
@Composable
private fun DeckColumns() {
    val context = LocalContext.current
    Prefs.observedVersion()
    val choices = listOf(2, 3, 4)
    val title = stringResource(R.string.settings_deck_columns)
    val summary = stringResource(R.string.settings_deck_columns_summary)
    if (!rememberSettingShown(title, summary)) return
    SettingRow(title, summary)
    ChoiceRow(
        null,
        choices.map { it.toString() },
        choices.indexOf(dev.pocketdesk.android.ui.Deck.columns(Prefs.deckColumns(context))),
        horizontal = 16.dp,
    ) { Prefs.setDeckColumns(context, choices[it]) }
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

/** Whether the phone can shake (P11's row is drawn only where it can). */
private fun hasVibrator(context: android.content.Context): Boolean =
    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
        context.getSystemService(android.os.VibratorManager::class.java)?.defaultVibrator?.hasVibrator() == true
    } else {
        @Suppress("DEPRECATION")
        context.getSystemService(android.os.Vibrator::class.java)?.hasVibrator() == true
    }

/**
 * Ecosystem phase 4: what "Offer my other computers after pairing" does,
 * and how the last introduction went.
 */
@Composable
private fun introduceSummary(context: android.content.Context): String {
    val summary = stringResource(R.string.settings_offer_my_computers_summary)
    val last = dev.pocketdesk.android.pair.IntroduceRule.decode(Prefs.introduceLast(context)) ?: return summary
    val said = when (last.kind) {
        dev.pocketdesk.android.pair.IntroduceRule.Kind.PAIRED ->
            stringResource(R.string.introduce_last_paired, last.name)
        dev.pocketdesk.android.pair.IntroduceRule.Kind.REFUSED ->
            stringResource(R.string.introduce_last_refused, last.name, last.code)
        dev.pocketdesk.android.pair.IntroduceRule.Kind.UNREACHABLE ->
            stringResource(R.string.introduce_last_unreachable, last.name)
        dev.pocketdesk.android.pair.IntroduceRule.Kind.EXPIRED ->
            stringResource(R.string.introduce_last_expired, last.name)
    }
    return summary + "\n" + stringResource(R.string.settings_introduce_last, said)
}
