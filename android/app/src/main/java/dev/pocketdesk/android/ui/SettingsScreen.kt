package dev.pocketdesk.android.ui

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.doctor.BackgroundLimit
import dev.pocketdesk.android.doctor.BackgroundRunning
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/** Appearance, session behavior and About — everything the old footer deep-link did, in one place. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    Prefs.observedVersion()

    @Composable
    fun boolRow(
        icon: LlGlyph,
        title: String,
        summary: String,
        checked: Boolean,
        onChange: (Boolean) -> Unit,
    ) {
        ListItem(
            headlineContent = { Text(title, style = MaterialTheme.typography.bodyLarge) },
            supportingContent = { Text(summary, style = MaterialTheme.typography.bodySmall) },
            leadingContent = { LlIcon(icon, null) },
            trailingContent = { Switch(checked = checked, onCheckedChange = onChange) },
            modifier = Modifier.clickable { onChange(!checked) },
        )
    }

    @Composable
    fun Header(label: String) {
        Text(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 2.dp),
        )
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_title)) },
                navigationIcon = {
                    LlIcon(
                        LlIcons.ArrowBack,
                        stringResource(R.string.settings_back),
                        modifier =
                            Modifier
                                .padding(start = 12.dp)
                                .clickable(onClick = onBack),
                    )
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
            Header(stringResource(R.string.settings_appearance))
            listOf(
                Prefs.THEME_SYSTEM to stringResource(R.string.theme_system),
                Prefs.THEME_LIGHT to stringResource(R.string.theme_light),
                Prefs.THEME_DARK to stringResource(R.string.theme_dark),
            ).forEach { (value, label) ->
                ListItem(
                    headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
                    leadingContent = {
                        RadioButton(
                            selected = Prefs.themeMode(context) == value,
                            onClick = { Prefs.setThemeMode(context, value) },
                        )
                    },
                    modifier = Modifier.clickable { Prefs.setThemeMode(context, value) },
                )
            }
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
                boolRow(
                    LlIcons.Desktop,
                    stringResource(R.string.settings_dynamic),
                    stringResource(R.string.settings_dynamic_summary),
                    Prefs.dynamicColor(context),
                ) { Prefs.setDynamicColor(context, it) }
            }

            Header(stringResource(R.string.settings_session))
            boolRow(
                LlIcons.Eye,
                stringResource(R.string.settings_keep_screen_on),
                stringResource(R.string.settings_keep_screen_on_summary),
                Prefs.keepScreenOn(context),
            ) { Prefs.setKeepScreenOn(context, it) }
            boolRow(
                LlIcons.Wifi,
                stringResource(R.string.settings_auto_reconnect),
                stringResource(R.string.settings_auto_reconnect_summary),
                Prefs.autoReconnect(context),
            ) { Prefs.setAutoReconnect(context, it) }
            boolRow(
                LlIcons.Wifi,
                stringResource(R.string.settings_retry_transfers),
                stringResource(R.string.settings_retry_transfers_summary),
                Prefs.retryTransfers(context),
            ) { Prefs.setRetryTransfers(context, it) }
            boolRow(
                LlIcons.Pin,
                stringResource(R.string.settings_others_choose),
                stringResource(R.string.settings_others_choose_summary),
                Prefs.othersChooseComputer(context),
            ) { Prefs.setOthersChooseComputer(context, it) }
            boolRow(
                LlIcons.Wifi,
                stringResource(R.string.settings_links_from_computer),
                stringResource(R.string.settings_links_from_computer_summary),
                Prefs.linksFromComputer(context),
            ) { Prefs.setLinksFromComputer(context, it) }
            boolRow(
                LlIcons.Wifi,
                stringResource(R.string.settings_share_battery),
                stringResource(R.string.settings_share_battery_summary),
                Prefs.shareBattery(context),
            ) { Prefs.setShareBattery(context, it) }
            boolRow(
                LlIcons.Mouse,
                stringResource(R.string.settings_capture_mouse),
                stringResource(R.string.settings_capture_mouse_summary),
                Prefs.captureMouse(context),
            ) { Prefs.setCaptureMouse(context, it) }
            boolRow(
                LlIcons.Volume,
                stringResource(R.string.settings_phone_media),
                stringResource(R.string.settings_phone_media_summary),
                Prefs.phoneMediaOnComputer(context),
            ) { on ->
                Prefs.setPhoneMediaOnComputer(context, on)
                // Android shows other apps' media only to an app with notification access.
                if (on && !PhoneNotificationPrefs.accessGranted(context)) {
                    runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
                }
            }
            boolRow(
                LlIcons.Pin,
                stringResource(R.string.settings_reveal_pin),
                stringResource(R.string.settings_reveal_pin_summary),
                Prefs.revealPin(context),
            ) { Prefs.setRevealPin(context, it) }
            boolRow(
                LlIcons.Touch,
                stringResource(R.string.settings_haptics),
                stringResource(R.string.settings_haptics_summary),
                Prefs.haptics(context),
            ) { Prefs.setHaptics(context, it) }
            boolRow(
                LlIcons.Touch,
                stringResource(R.string.settings_show_driving),
                stringResource(R.string.settings_show_driving_summary),
                Prefs.showDriving(context),
            ) { Prefs.setShowDriving(context, it) }
            // F3: trackpad speed, saved when the thumb is let go.
            var speed by remember { mutableFloatStateOf(Prefs.trackpadSensitivity(context)) }
            ListItem(
                headlineContent = {
                    Text(stringResource(R.string.settings_trackpad_speed), style = MaterialTheme.typography.bodyLarge)
                },
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
                        )
                    }
                },
                leadingContent = { LlIcon(LlIcons.Mouse, null) },
                trailingContent = { Text("%.2f×".format(speed), style = MaterialTheme.typography.labelLarge) },
            )

            // N6: what Wake does once the packet is out. How each computer
            // is woken is set on its card ("Wake settings").
            Header(stringResource(R.string.settings_wake))
            boolRow(
                LlIcons.Desktop,
                stringResource(R.string.settings_connect_after_wake),
                stringResource(R.string.settings_connect_after_wake_summary),
                Prefs.connectAfterWake(context),
            ) { Prefs.setConnectAfterWake(context, it) }
            boolRow(
                LlIcons.Desktop,
                stringResource(R.string.settings_offer_wake),
                stringResource(R.string.settings_offer_wake_summary),
                Prefs.offerWake(context),
            ) { Prefs.setOfferWake(context, it) }

            PhoneNotificationsSection()
            TextsSection()

            // T2: whether Android holds those back while the phone sleeps.
            Header(stringResource(R.string.settings_background))
            BackgroundSection()
            boolRow(
                LlIcons.Notifications,
                stringResource(R.string.settings_background_remind),
                stringResource(R.string.settings_background_remind_summary),
                Prefs.backgroundReminder(context),
            ) { Prefs.setBackgroundReminder(context, it) }

            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
                Header(stringResource(R.string.settings_system))
                ListItem(
                    headlineContent = { Text(stringResource(R.string.language), style = MaterialTheme.typography.bodyLarge) },
                    leadingContent = { LlIcon(LlIcons.Language, null) },
                    trailingContent = { LlIcon(LlIcons.ChevronRight, null) },
                    modifier =
                        Modifier.clickable {
                            runCatching {
                                context.startActivity(
                                    Intent(
                                        Settings.ACTION_APP_LOCALE_SETTINGS,
                                        ("package:${context.packageName}").toUri(),
                                    ),
                                )
                            }
                        },
                )
            }

            Header(stringResource(R.string.settings_about))
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
                leadingContent = { LlIcon(LlIcons.Info, null) },
            )
        }
    }
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
    ListItem(
        headlineContent = {
            Text(stringResource(R.string.settings_background_status), style = MaterialTheme.typography.bodyLarge)
        },
        supportingContent = {
            Text(
                stringResource(
                    when (limit) {
                        BackgroundLimit.NONE -> R.string.bg_allowed
                        BackgroundLimit.OPTIMIZED -> R.string.bg_optimized
                        BackgroundLimit.RESTRICTED -> R.string.bg_restricted
                    },
                ),
                style = MaterialTheme.typography.bodySmall,
            )
        },
        leadingContent = { LlIcon(LlIcons.Settings, null) },
        trailingContent = { LlIcon(LlIcons.ChevronRight, null) },
        modifier = Modifier.clickable { BackgroundRunning.openSettings(context, limit) },
    )
    if (guide != null) {
        ListItem(
            headlineContent = {
                Text(stringResource(R.string.bg_maker_guide), style = MaterialTheme.typography.bodyLarge)
            },
            supportingContent = {
                Text(stringResource(R.string.bg_maker_guide_summary), style = MaterialTheme.typography.bodySmall)
            },
            leadingContent = { LlIcon(LlIcons.Info, null) },
            trailingContent = { LlIcon(LlIcons.ChevronRight, null) },
            modifier = Modifier.clickable {
                if (!BackgroundRunning.openGuide(context, guide)) {
                    android.widget.Toast.makeText(context, guide, android.widget.Toast.LENGTH_LONG).show()
                }
            },
        )
    }
}
