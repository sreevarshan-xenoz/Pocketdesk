package dev.pocketdesk.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore

/**
 * The session's command centre, in groups (U4): Screen and input, Share,
 * Computer, This phone. Input mode, Quality and Pane are segmented choices
 * that show every option; switches flip in place and keep the sheet open;
 * rows that open a sheet or picker carry a chevron and dismiss first, and
 * rows that act at once (ring, lock, blackout, picture-in-picture) carry
 * none.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun QuickSettingsSheet(
    onDismiss: () -> Unit,
    trackpad: Boolean,
    onSetTrackpad: (Boolean) -> Unit,
    viewOnly: Boolean,
    onSetViewOnly: (Boolean) -> Unit,
    qualityPreset: Int,
    onSetQuality: (Int) -> Unit,
    windowValue: String,
    onPickWindow: () -> Unit,
    monitorValue: String,
    onPickMonitor: () -> Unit,
    paneMode: PaneMode,
    onSetPane: (PaneMode) -> Unit,
    desktopSound: Boolean,
    onSetDesktopSound: (Boolean) -> Unit,
    clipboardSync: Boolean,
    onSetClipboardSync: (Boolean) -> Unit,
    onOpenHistory: () -> Unit,
    onOpenAudio: () -> Unit,
    onSendText: () -> Unit,
    onTypeClipboard: () -> Unit,
    onOpenFiles: () -> Unit,
    onOpenTasks: () -> Unit,
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
                modifier = Modifier
                    .padding(start = 8.dp, bottom = 8.dp)
                    .semantics { heading() },
            )

            GroupHeader(stringResource(R.string.group_view))
            ChoiceRow(
                stringResource(R.string.row_input_mode),
                listOf(stringResource(R.string.value_direct_touch), stringResource(R.string.value_trackpad)),
                if (trackpad) 1 else 0,
            ) { onSetTrackpad(it == 1) }
            ToggleRow(LlIcons.Eye, stringResource(R.string.row_view_only), viewOnly, onSetViewOnly)
            val presets = listOf(
                RustCore.PRESET_AUTO,
                RustCore.PRESET_QUALITY,
                RustCore.PRESET_BALANCED,
                RustCore.PRESET_ECONOMY,
            )
            ChoiceRow(
                stringResource(R.string.row_quality),
                listOf(
                    stringResource(R.string.value_quality_auto),
                    stringResource(R.string.value_quality_max),
                    stringResource(R.string.value_quality_balanced),
                    stringResource(R.string.value_quality_economy),
                ),
                presets.indexOf(qualityPreset).coerceAtLeast(0),
            ) { onSetQuality(presets[it]) }
            ChoiceRow(
                stringResource(R.string.row_pane),
                listOf(
                    stringResource(R.string.value_pane_auto),
                    stringResource(R.string.value_pane_dual),
                    stringResource(R.string.value_pane_single),
                ),
                PaneMode.entries.indexOf(paneMode),
            ) { onSetPane(PaneMode.entries[it]) }
            OpenRow(LlIcons.Desktop, stringResource(R.string.row_window), windowValue, onPickWindow)
            OpenRow(LlIcons.Monitor, stringResource(R.string.row_monitor), monitorValue, onPickMonitor)

            GroupHeader(stringResource(R.string.group_share))
            ToggleRow(LlIcons.Clipboard, stringResource(R.string.row_clipboard), clipboardSync, onSetClipboardSync)
            OpenRow(LlIcons.History, stringResource(R.string.row_history), null, onOpenHistory)
            OpenRow(LlIcons.Keyboard, stringResource(R.string.row_send_text), null, onSendText)
            ActionRow(LlIcons.Clipboard, stringResource(R.string.row_type_clipboard), onTypeClipboard)
            OpenRow(LlIcons.Folder, stringResource(R.string.row_files), null, onOpenFiles)
            OpenRow(LlIcons.Play, stringResource(R.string.row_tasks), null, onOpenTasks)
            ToggleRow(LlIcons.Mic, stringResource(R.string.row_mic), micOn, onSetMic)

            GroupHeader(stringResource(R.string.group_computer))
            OpenRow(LlIcons.Volume, stringResource(R.string.row_audio), null, onOpenAudio)
            ActionRow(LlIcons.Notifications, stringResource(R.string.ring_pc), onRing)
            ActionRow(LlIcons.Power, stringResource(R.string.lock_pc), onLock)
            ActionRow(LlIcons.Moon, stringResource(R.string.blackout), onBlackout)
            ToggleRow(LlIcons.Lock, stringResource(R.string.row_privacy), privacyGrab, onSetPrivacy)

            GroupHeader(stringResource(R.string.settings_this_phone))
            ToggleRow(LlIcons.Volume, stringResource(R.string.row_desktop_sound), desktopSound, onSetDesktopSound)
            OpenRow(
                LlIcons.Pin,
                stringResource(R.string.row_pairing),
                stringResource(if (paired) R.string.value_paired else R.string.value_unpaired),
                onOpenPairing,
            )
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
        modifier = Modifier
            .padding(start = 8.dp, top = 16.dp, bottom = 2.dp)
            .semantics { heading() },
    )
}

/** Every option at once, the chosen one filled; a tap chooses, with no cycling. */
@Composable
internal fun ChoiceRow(label: String, options: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    Column(modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp)) {
        Text(
            label,
            style = MaterialTheme.typography.bodyLarge,
            modifier = Modifier.padding(start = 8.dp, bottom = 6.dp),
        )
        // Four options leave little room ("Equilibrada", "அதிகபட்சம்"):
        // no check (the fill says which), narrower sides, a smaller face,
        // and two lines before an ellipsis (U4 review).
        val crowded = options.size > 3
        val face = if (crowded) MaterialTheme.typography.labelMedium else LocalTextStyle.current
        BoxWithConstraints(modifier = Modifier.fillMaxWidth()) {
            // C5: a segment narrower than one of its words (font scale 2.0)
            // broke it mid-word; such a row is a list instead.
            val measurer = rememberTextMeasurer()
            val density = LocalDensity.current
            val sides = if (crowded) 8.dp else 48.dp
            val segment = with(density) { (maxWidth / options.size - sides).toPx() }
            val widths = remember(options, face, density, segment) {
                // Four wrap between words; up to three keep each on one line.
                options.flatMap { if (crowded) ChoiceFit.words(it) else listOf(it) }
                    .map { measurer.measure(it, face).size.width.toFloat() }
            }
            if (ChoiceFit.fits(segment, widths)) {
                Segments(options, selected, crowded, face, onSelect)
            } else {
                ChoiceList(options, selected, onSelect)
            }
        }
    }
}

/** [options] as one row of segments, the [selected] one filled. */
@Composable
private fun Segments(
    options: List<String>,
    selected: Int,
    crowded: Boolean,
    face: androidx.compose.ui.text.TextStyle,
    onSelect: (Int) -> Unit,
) {
    SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
        options.forEachIndexed { i, option ->
            SegmentedButton(
                selected = i == selected,
                onClick = { onSelect(i) },
                shape = SegmentedButtonDefaults.itemShape(index = i, count = options.size),
                icon = if (crowded) ({}) else ({ SegmentedButtonDefaults.Icon(i == selected) }),
                contentPadding = if (crowded) PaddingValues(horizontal = 4.dp) else SegmentedButtonDefaults.ContentPadding,
            ) {
                Text(
                    option,
                    style = face,
                    maxLines = if (crowded) 2 else 1,
                    overflow = TextOverflow.Ellipsis,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

/** [options] as radio buttons, one per line, when they do not fit as segments (C5). */
@Composable
private fun ChoiceList(options: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    Column(modifier = Modifier.selectableGroup()) {
        options.forEachIndexed { i, option ->
            ListItem(
                headlineContent = { Text(option, style = MaterialTheme.typography.bodyLarge) },
                leadingContent = { RadioButton(selected = i == selected, onClick = null) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.selectable(selected = i == selected, role = Role.RadioButton) { onSelect(i) },
            )
        }
    }
}

/** One toggle and one TalkBack stop for the whole row. */
@Composable
private fun ToggleRow(icon: LlGlyph, label: String, checked: Boolean, onToggle: (Boolean) -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        // The sheet's own colour: the list's default painted bands across it.
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        trailingContent = { Switch(checked = checked, onCheckedChange = null) },
        modifier = Modifier.toggleable(value = checked, role = Role.Switch, onValueChange = onToggle),
    )
}

/** A row that opens a sheet or picker: its current value, if it has one, and a chevron. */
@Composable
private fun OpenRow(icon: LlGlyph, label: String, value: String?, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        // The sheet's own colour: the list's default painted bands across it.
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        trailingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (value != null) {
                    Text(
                        value,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.widthIn(max = 130.dp),
                    )
                }
                LlIcon(LlIcons.ChevronRight, null)
            }
        },
        modifier = Modifier.clickable(role = Role.Button, onClick = onClick),
    )
}

/** A row that acts at once: no chevron, which would promise a page. */
@Composable
private fun ActionRow(icon: LlGlyph, label: String, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { LlIcon(icon, null) },
        // The sheet's own colour: the list's default painted bands across it.
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.clickable(role = Role.Button, onClick = onClick),
    )
}
