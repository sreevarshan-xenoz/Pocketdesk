package dev.pocketdesk.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.pocketdesk.android.R

/*
 * The parts every Settings row is made of (U4): a group's heading, marked as
 * one for TalkBack; rows with no icon (they carried no meaning); a summary of
 * at most two lines, with an ⓘ that shows the whole when it is longer; and a
 * switch row that is one toggle and one TalkBack stop (a long summary's ⓘ is
 * a second, its own button).
 */

/**
 * O8: what Settings' search box holds, and which rows each group's search
 * found (a group's heading shows while one is). A new one per search.
 */
class SettingsFilter(val query: String) {
    val found = mutableStateMapOf<String, Set<Any>>()

    /** Whether nothing a search could find was found: one frame behind the rows. */
    fun nothingFound(): Boolean = SettingsSearch.searching(query) && found.values.all { it.isEmpty() }
}

val LocalSettingsFilter = compositionLocalOf { SettingsFilter("") }
private val LocalSettingsGroup = compositionLocalOf { "" }

/** A group: its heading, shown unless a search found nothing in it, then its rows. */
@Composable
fun SettingsGroup(label: String, content: @Composable () -> Unit) {
    val filter = LocalSettingsFilter.current
    if (SettingsSearch.groupShown(filter.query, filter.found[label].orEmpty().size)) SettingsHeader(label)
    CompositionLocalProvider(LocalSettingsGroup provides label) { content() }
}

/**
 * O8: whether a row of [texts] shows for the search, reported to its group
 * so the group's heading shows with it.
 */
@Composable
fun rememberSettingShown(vararg texts: String?): Boolean {
    val filter = LocalSettingsFilter.current
    val group = LocalSettingsGroup.current
    val shown = SettingsSearch.rowShown(filter.query, texts.filterNotNull(), group)
    val key = remember { Any() }
    DisposableEffect(filter, group, shown) {
        if (shown && SettingsSearch.searching(filter.query)) filter.found[group] = filter.found[group].orEmpty() + key
        onDispose { filter.found[group]?.let { filter.found[group] = it - key } }
    }
    return shown
}

/** O8: a block that is not a [SettingRow] (a field, a slider, a choice), shown as a row of [texts] is. */
@Composable
fun SettingsBlock(vararg texts: String?, content: @Composable () -> Unit) {
    if (rememberSettingShown(*texts)) content()
}

/** A group's heading. */
@Composable
fun SettingsHeader(label: String) {
    Text(
        label,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier
            .padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 4.dp)
            .semantics { heading() },
    )
}

/**
 * A row with a switch, which the whole row toggles, an [indent] included.
 * [warning] shows the summary in the error colour.
 */
@Composable
fun SettingSwitch(
    title: String,
    summary: String?,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
    enabled: Boolean = true,
    warning: Boolean = false,
    indent: Boolean = false,
    nested: Boolean = false,
) {
    SettingRow(
        title = title,
        summary = summary,
        warning = warning,
        // The indent inside the toggle, so it is no dead strip (U4 review);
        // a nested row (O5's kinds under their app) twice as deep.
        modifier = Modifier
            .toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = onChange)
            .padding(start = if (nested) 48.dp else if (indent) 24.dp else 0.dp),
    ) {
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

/** A row that opens something: a page of Android's, a guide. */
@Composable
fun SettingLink(title: String, summary: String?, warning: Boolean = false, onClick: () -> Unit) {
    SettingRow(
        title = title,
        summary = summary,
        modifier = Modifier.clickable(role = Role.Button, onClick = onClick),
        warning = warning,
    ) {
        LlIcon(LlIcons.ChevronRight, null)
    }
}

/**
 * A row's title, its summary cut at two lines with an ⓘ for the whole, and
 * what goes at its end.
 */
@Composable
fun SettingRow(
    title: String,
    summary: String?,
    modifier: Modifier = Modifier,
    warning: Boolean = false,
    trailing: (@Composable () -> Unit)? = null,
) {
    // O8: a row the search does not find is not drawn.
    if (!rememberSettingShown(title, summary)) return
    var long by remember(summary) { mutableStateOf(false) }
    var showing by remember { mutableStateOf(false) }
    ListItem(
        headlineContent = { Text(title, style = MaterialTheme.typography.bodyLarge) },
        supportingContent = summary?.let {
            {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (warning) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    onTextLayout = { layout -> if (layout.hasVisualOverflow) long = true },
                )
            }
        },
        trailingContent = if (long || trailing != null) {
            {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (long) {
                        IconButton(onClick = { showing = true }) {
                            LlIcon(LlIcons.Info, stringResource(R.string.settings_more_about, title), size = 20.dp)
                        }
                    }
                    trailing?.invoke()
                }
            }
        } else {
            null
        },
        modifier = modifier,
    )
    if (showing && summary != null) {
        AlertDialog(
            onDismissRequest = { showing = false },
            title = { Text(title) },
            text = { Text(summary) },
            confirmButton = {
                TextButton(onClick = { showing = false }) { Text(stringResource(R.string.settings_info_close)) }
            },
        )
    }
}

/**
 * How many times the screen has come back to the front: a section reads its
 * state again under it, since Android's own pages (notification access, a
 * permission) may have changed it meanwhile (U4 review).
 */
@Composable
fun rememberResumes(): Int {
    val owner = LocalLifecycleOwner.current
    var resumes by remember { mutableIntStateOf(0) }
    DisposableEffect(owner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) resumes++ }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    return resumes
}

