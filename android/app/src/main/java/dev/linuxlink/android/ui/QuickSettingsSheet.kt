package dev.linuxlink.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.linuxlink.android.R

/**
 * The session's command centre: everything the old 16-button bottom bar did,
 * grouped in one dismissable sheet so the remote desktop itself stays clean.
 * Toggle rows keep the sheet open (the state flips in place); rows that lead
 * elsewhere (pickers, pairing, blackout, PiP) dismiss first.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun QuickSettingsSheet(
    onDismiss: () -> Unit,
    inputModeValue: String,
    onCycleInputMode: () -> Unit,
    viewOnly: Boolean,
    onSetViewOnly: (Boolean) -> Unit,
    qualityValue: String,
    onCycleQuality: () -> Unit,
    windowValue: String,
    onPickWindow: () -> Unit,
    monitorValue: String,
    onPickMonitor: () -> Unit,
    paneValue: String,
    onCyclePane: () -> Unit,
    desktopSound: Boolean,
    onSetDesktopSound: (Boolean) -> Unit,
    clipboardSync: Boolean,
    onSetClipboardSync: (Boolean) -> Unit,
    onOpenHistory: () -> Unit,
    onOpenAudio: () -> Unit,
    onSendText: () -> Unit,
    onTypeClipboard: () -> Unit,
    onOpenFiles: () -> Unit,
    micOn: Boolean,
    onSetMic: (Boolean) -> Unit,
    privacyGrab: Boolean,
    onSetPrivacy: (Boolean) -> Unit,
    paired: Boolean,
    onOpenPairing: () -> Unit,
    onRing: () -> Unit,
    onLock: () -> Unit,
    onBlackout: () -> Unit,
    onPip: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 8.dp)
                .padding(bottom = 24.dp),
        ) {
            Text(
                stringResource(R.string.quick_settings_title),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(start = 8.dp, bottom = 8.dp),
            )

            GroupHeader(stringResource(R.string.group_input))
            ValueRow(LlIcons.Keyboard, stringResource(R.string.row_input_mode), inputModeValue, onCycleInputMode)
            ToggleRow(LlIcons.Eye, stringResource(R.string.row_view_only), viewOnly, onSetViewOnly)

            GroupHeader(stringResource(R.string.group_stream))
            ValueRow(LlIcons.Wifi, stringResource(R.string.row_quality), qualityValue, onCycleQuality)
            ValueRow(LlIcons.Desktop, stringResource(R.string.row_window), windowValue, onPickWindow)
            ValueRow(LlIcons.Monitor, stringResource(R.string.row_monitor), monitorValue, onPickMonitor)
            ValueRow(LlIcons.Lan, stringResource(R.string.row_pane), paneValue, onCyclePane)
            ToggleRow(LlIcons.Volume, stringResource(R.string.row_desktop_sound), desktopSound, onSetDesktopSound)

            GroupHeader(stringResource(R.string.group_desktop))
            ToggleRow(LlIcons.Clipboard, stringResource(R.string.row_clipboard), clipboardSync, onSetClipboardSync)
            ActionRow(LlIcons.History, stringResource(R.string.row_history), onOpenHistory)
            ActionRow(LlIcons.Volume, stringResource(R.string.row_audio), onOpenAudio)
            ActionRow(LlIcons.Keyboard, stringResource(R.string.row_send_text), onSendText)
            ActionRow(LlIcons.Clipboard, stringResource(R.string.row_type_clipboard), onTypeClipboard)
            ActionRow(LlIcons.Folder, stringResource(R.string.row_files), onOpenFiles)
            ToggleRow(LlIcons.Mic, stringResource(R.string.row_mic), micOn, onSetMic)
            ToggleRow(LlIcons.Lock, stringResource(R.string.row_privacy), privacyGrab, onSetPrivacy)
            ValueRow(
                LlIcons.Pin,
                stringResource(R.string.row_pairing),
                stringResource(if (paired) R.string.value_paired else R.string.value_unpaired),
                onOpenPairing,
            )
            ActionRow(LlIcons.Notifications, stringResource(R.string.ring_pc), onRing)
            ActionRow(LlIcons.Power, stringResource(R.string.lock_pc), onLock)
            ActionRow(LlIcons.Moon, stringResource(R.string.blackout), onBlackout)
            ActionRow(LlIcons.Pip, stringResource(R.string.pip), onPip)
        }
    }
}

@Composable
private fun GroupHeader(label: String) {
    Text(
        label,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 8.dp, top = 12.dp, bottom = 2.dp),
    )
}

@Composable
private fun ToggleRow(icon: LlGlyph, label: String, checked: Boolean, onToggle: (Boolean) -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        trailingContent = { Switch(checked = checked, onCheckedChange = onToggle) },
        modifier = Modifier.clickable { onToggle(!checked) },
    )
}

@Composable
private fun ValueRow(icon: LlGlyph, label: String, value: String, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        trailingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    value,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.widthIn(max = 130.dp),
                )
                LlIcon(LlIcons.ChevronRight, null)
            }
        },
        modifier = Modifier.clickable(onClick = onClick),
    )
}

@Composable
private fun ActionRow(icon: LlGlyph, label: String, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        trailingContent = { LlIcon(LlIcons.ChevronRight, null) },
        modifier = Modifier.clickable(onClick = onClick),
    )
}
