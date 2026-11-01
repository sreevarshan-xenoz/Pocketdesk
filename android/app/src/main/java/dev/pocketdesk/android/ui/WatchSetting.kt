package dev.pocketdesk.android.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.watch.WatchLinkServer
import dev.pocketdesk.android.watch.WatchStore
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchPairing
import kotlinx.coroutines.delay

/**
 * Watch controls (item 9): a paired Wear OS watch may wake the computer,
 * ring this phone, play the computer's music, press its deck buttons and say
 * No to its questions, each its own switch. Off until chosen; turning it on
 * asks for Android's nearby-devices permission once, for a Bluetooth link to
 * a watch already paired with this phone. A watch has no Yes: a question that
 * needs this phone's fingerprint is answered here.
 */
@Composable
fun WatchSetting() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.watchControls(context) }
    val granted = (version + resumes).let { WatchLinkServer.hasPermission(context) }
    val listening = (version + resumes).let { PhoneNotificationPrefs.accessGranted(context) }
    val paired = (version + resumes).let { WatchStore.paired(context) }
    var pairing by remember { mutableStateOf(false) }

    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        Prefs.setWatchControls(context, ok)
        if (ok) WatchLinkServer.sync(context)
        version++
    }

    val active = on && granted
    SettingSwitch(
        stringResource(R.string.settings_watch),
        stringResource(
            when {
                active && !listening -> R.string.settings_watch_needs_access
                else -> R.string.settings_watch_summary
            },
        ),
        active,
        { value ->
            if (value && !granted) {
                ask.launch(android.Manifest.permission.BLUETOOTH_CONNECT)
            } else {
                Prefs.setWatchControls(context, value)
                if (value) WatchLinkServer.sync(context) else WatchLinkServer.stop()
                version++
            }
        },
        warning = active && !listening,
    )
    if (!active) return

    if (paired == null) {
        SettingLink(
            stringResource(R.string.settings_watch_pair),
            stringResource(R.string.settings_watch_pair_summary),
        ) { pairing = true }
    } else {
        SettingRow(
            stringResource(R.string.settings_watch_paired, paired.name.ifBlank { stringResource(R.string.settings_watch_unnamed) }),
            stringResource(R.string.settings_watch_paired_summary),
            trailing = {
                TextButton(onClick = {
                    WatchStore.forget(context)
                    version++
                }) { Text(stringResource(R.string.settings_watch_forget)) }
            },
        )
    }

    val chosen = (version + resumes).let { Prefs.watchButtons(context) }
    for ((button, labels) in buttonLabels) {
        SettingSwitch(
            stringResource(labels.first),
            stringResource(labels.second),
            button in chosen,
            { value ->
                Prefs.setWatchButtons(context, if (value) chosen + button else chosen - button)
                version++
            },
            indent = true,
        )
    }

    if (pairing) {
        PairDialog(onClose = {
            pairing = false
            version++
        })
    }
}

/** Each button's title and summary. */
private val buttonLabels = listOf(
    WatchButton.Wake to (R.string.settings_watch_wake to R.string.settings_watch_wake_summary),
    WatchButton.Ring to (R.string.settings_watch_ring to R.string.settings_watch_ring_summary),
    WatchButton.Music to (R.string.settings_watch_music to R.string.settings_watch_music_summary),
    WatchButton.Deck to (R.string.settings_watch_deck to R.string.settings_watch_deck_summary),
    WatchButton.Asks to (R.string.settings_watch_asks to R.string.settings_watch_asks_summary),
)

/** The pairing screen: six digits read off the watch, typed here; the window is the phone's for two minutes. */
@Composable
private fun PairDialog(onClose: () -> Unit) {
    val context = LocalContext.current
    var code by remember { mutableStateOf("") }
    var said by remember { mutableStateOf<Int?>(null) }
    var done by remember { mutableStateOf(false) }

    DisposableEffect(Unit) {
        WatchLinkServer.pairing.open()
        WatchLinkServer.sync(context)
        onDispose { WatchLinkServer.pairing.close() }
    }
    // The watch's answer arrives on the link's own thread; the pairing is seen when it is saved.
    LaunchedEffect(Unit) {
        val start = System.currentTimeMillis()
        while (System.currentTimeMillis() - start < 130_000L) {
            if (WatchStore.paired(context) != null) {
                done = true
                return@LaunchedEffect
            }
            if (!WatchLinkServer.pairing.isOpen && said == null) said = R.string.settings_watch_pair_closed
            delay(500)
        }
    }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(stringResource(R.string.settings_watch_pair)) },
        text = {
            androidx.compose.foundation.layout.Column {
                Text(stringResource(if (done) R.string.settings_watch_pair_done else R.string.settings_watch_pair_steps))
                if (!done) {
                    OutlinedTextField(
                        value = code,
                        onValueChange = {
                            code = it.filter { c -> c.isDigit() }.take(WatchPairing.CODE_DIGITS)
                            said = null
                        },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                        label = { Text(stringResource(R.string.settings_watch_pair_code)) },
                    )
                    said?.let { Text(stringResource(it)) }
                }
            }
        },
        confirmButton = {
            if (done) {
                TextButton(onClick = onClose) { Text(stringResource(R.string.settings_info_close)) }
            } else {
                TextButton(
                    enabled = WatchPairing.isCode(code),
                    onClick = {
                        said = if (WatchLinkServer.pairing.offer(code)) {
                            R.string.settings_watch_pair_waiting
                        } else {
                            R.string.settings_watch_pair_closed
                        }
                    },
                ) { Text(stringResource(R.string.settings_watch_pair_go)) }
            }
        },
        dismissButton = {
            if (!done) TextButton(onClick = onClose) { Text(stringResource(R.string.settings_watch_pair_cancel)) }
        },
    )
}
