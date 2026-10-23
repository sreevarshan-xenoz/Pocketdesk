package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.notify.NotificationKinds
import dev.pocketdesk.android.notify.PhoneNotificationPrefs

/**
 * Settings: whether this phone's notifications appear on the computer (N5a),
 * off by default, and which apps share theirs: each app that has notified is
 * listed, off until switched on, so no app's notification reaches the
 * computer before the user chose it. Read again on resume, so a grant made in
 * Android's settings shows when the user comes back.
 */
@Composable
fun PhoneNotificationsSection() {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var version by remember { mutableIntStateOf(0) }
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) version++ }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    // Read under `version`, so each change and each resume recomposes.
    val enabled = version.let { PhoneNotificationPrefs.enabled(context) }
    val granted = version.let { PhoneNotificationPrefs.accessGranted(context) }
    val shared = version.let { PhoneNotificationPrefs.shared(context) }
    val seen = version.let { PhoneNotificationPrefs.seen(context) }

    fun setEnabled(on: Boolean) {
        PhoneNotificationPrefs.setEnabled(context, on)
        if (on && !PhoneNotificationPrefs.accessGranted(context)) {
            runCatching { context.startActivity(PhoneNotificationPrefs.accessSettingsIntent()) }
        }
        version++
    }

    val answer = version.let { Prefs.answerNotifications(context) }

    SettingSwitch(
        stringResource(R.string.phone_notifications_share),
        stringResource(if (enabled && !granted) R.string.phone_notifications_no_access else R.string.phone_notifications_share_summary),
        enabled,
        ::setEnabled,
        warning = enabled && !granted,
    )
    if (enabled && !granted) AllowNotificationAccess()
    // Its neighbours go while sharing is off, and so does it (E6); a search
    // still finds them, greyed (Q3 review).
    UnderSwitch(enabled) {
    // L1: closed on the computer, cleared here.
    SettingSwitch(
        stringResource(R.string.phone_notifications_clear),
        stringResource(R.string.phone_notifications_clear_summary),
        version.let { Prefs.letComputerClear(context) },
        {
            Prefs.setLetComputerClear(context, it)
            version++
        },
    )
    // N4: a shared app's progress and timers, one card each on the computer.
    SettingSwitch(
        stringResource(R.string.phone_notifications_live),
        stringResource(R.string.phone_notifications_live_summary),
        version.let { Prefs.liveUpdates(context) },
        {
            Prefs.setLiveUpdates(context, it)
            version++
        },
    )
    // N3: the computer may open a notification here, as a tap on it would.
    SettingSwitch(
        stringResource(R.string.phone_notifications_open),
        stringResource(R.string.phone_notifications_open_summary),
        version.let { Prefs.letComputerOpen(context) },
        {
            Prefs.setLetComputerOpen(context, it)
            version++
        },
    )
    // T10c: the computer may press buttons and reply, as this phone's owner.
    SettingSwitch(
        stringResource(R.string.phone_notifications_answer),
        stringResource(R.string.phone_notifications_answer_summary),
        answer,
        {
            Prefs.setAnswerNotifications(context, it)
            version++
        },
    )
    // M6: no pop-up on the computer while this phone is in use.
    val inHand = version.let { Prefs.quietInHand(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_in_hand),
        stringResource(R.string.phone_notifications_in_hand_summary),
        inHand,
        {
            Prefs.setQuietInHand(context, it)
            version++
        },
    )
    // P5: a ringing alarm or finished timer rings on the computer too.
    val alarms = version.let { Prefs.alarmsOnComputer(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_alarms),
        stringResource(R.string.phone_notifications_alarms_summary),
        alarms,
        {
            Prefs.setAlarmsOnComputer(context, it)
            version++
        },
    )
    // N7: a pop-up only on the computer in use.
    val toastWhere = version.let { Prefs.toastWhereWorking(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_toast_where),
        stringResource(R.string.phone_notifications_toast_where_summary),
        toastWhere,
        {
            Prefs.setToastWhereWorking(context, it)
            version++
        },
    )
    // K3: a notification's picture with it, off by default.
    val pictures = version.let { Prefs.notificationPictures(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_pictures),
        stringResource(R.string.phone_notifications_pictures_summary),
        pictures,
        {
            Prefs.setNotificationPictures(context, it)
            version++
        },
    )
    // D2: what Do Not Disturb hides on the phone stays off the computer.
    val holdBack = version.let { dev.pocketdesk.android.notify.QuietPrefs.holdBack(context) }
    SettingSwitch(
        stringResource(R.string.phone_notifications_hold_back),
        stringResource(R.string.phone_notifications_hold_back_summary),
        holdBack,
        {
            dev.pocketdesk.android.notify.QuietPrefs.setHoldBack(context, it)
            version++
        },
    )
    // C3: how long a notification waits for a computer out of reach.
    val wait = version.let { PhoneNotificationPrefs.waitMinutes(context) }
    SettingRow(stringResource(R.string.phone_notifications_wait), stringResource(R.string.phone_notifications_wait_summary))
    // Its choices hide with it (O8 review).
    SettingsBlock(stringResource(R.string.phone_notifications_wait), stringResource(R.string.phone_notifications_wait_summary)) {
    Column(Modifier.selectableGroup()) {
        PhoneNotificationPrefs.WAIT_CHOICES.forEach { minutes ->
            val label = when (minutes) {
                60 -> R.string.phone_notifications_wait_60
                10 -> R.string.phone_notifications_wait_10
                else -> R.string.phone_notifications_wait_0
            }
            ListItem(
                headlineContent = { Text(stringResource(label), style = MaterialTheme.typography.bodyLarge) },
                leadingContent = { RadioButton(selected = wait == minutes, onClick = null) },
                modifier = Modifier
                    .selectable(selected = wait == minutes, role = Role.RadioButton) {
                        PhoneNotificationPrefs.setWaitMinutes(context, minutes)
                        version++
                    }
                    .padding(start = 24.dp),
            )
        }
    }
    }
    if (seen.isEmpty()) {
        // O8: shown as a row is, for the search.
        SettingsBlock(stringResource(R.string.phone_notifications_apps_empty)) {
            Text(
                stringResource(R.string.phone_notifications_apps_empty),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            )
        }
    }
    seen.forEach { (pkg, label) ->
        SettingSwitch(
            label,
            pkg,
            pkg in shared,
            {
                PhoneNotificationPrefs.setShared(context, pkg, it)
                version++
            },
            indent = true,
        )
        // O5: a shared app's kinds, each one that can stay on the phone.
        if (pkg in shared) {
            val kept = version.let { PhoneNotificationPrefs.keptKinds(context, pkg) }
            val kinds = version.let { NotificationKinds.listed(PhoneNotificationPrefs.kinds(context, pkg), kept) }
            if (kinds.size > 1 || kept.isNotEmpty()) {
                kinds.forEach { kind ->
                    // Its app named beside its kind, for TalkBack too (O5 review).
                    SettingSwitch(
                        kind.example.ifBlank { stringResource(R.string.phone_notifications_kind_untitled) },
                        stringResource(R.string.phone_notifications_kind, label, kind.id),
                        kind.id !in kept,
                        {
                            PhoneNotificationPrefs.setKindKept(context, pkg, kind.id, !it)
                            version++
                        },
                        nested = true,
                    )
                }
            }
        }
    }
    // L8: other computers of mine get a copy, and while linked may answer it (part 2);
    // after the app list, so its switches keep their heading (L8 review).
    // Every paired computer is listed, the own one too: its pick counts
    // when another is the one in use (L8 review).
    val own = version.let { dev.pocketdesk.android.notify.PhoneLink.target(context)?.address }
    val computers = version.let {
        dev.pocketdesk.android.HostStore.hosts(context).mapNotNull { host ->
            dev.pocketdesk.android.HostStore.pairedDesktopId(context, host.address)?.let { host to it }
        }.distinctBy { it.second }
    }
    if (computers.size > 1) {
        val picked = version.let { dev.pocketdesk.android.notify.AlsoToPrefs.picked(context) }
        val more = dev.pocketdesk.android.notify.AlsoTo.mayPickMore(picked.size)
        SettingRow(
            stringResource(R.string.phone_notifications_also),
            stringResource(R.string.phone_notifications_also_summary),
        )
        computers.forEach { (host, desktopId) ->
            val on = desktopId in picked
            SettingSwitch(
                stringResource(
                    R.string.phone_notifications_also_to,
                    dev.pocketdesk.android.HostStore.shownName(context, host.address),
                ),
                if (host.address == own) stringResource(R.string.phone_notifications_also_own) else null,
                on,
                {
                    dev.pocketdesk.android.notify.AlsoToPrefs.set(context, desktopId, it)
                    version++
                },
                enabled = on || more,
            )
        }
    }
    }
}
