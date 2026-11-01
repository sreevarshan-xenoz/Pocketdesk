package dev.pocketdesk.android.ui

import android.net.Uri
import android.provider.DocumentsContract
import android.text.format.DateUtils
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ListItem
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.files.KeptFolders
import dev.pocketdesk.android.files.KeptRule

/**
 * Settings "Folders kept on my computer": each folder picked with Android's
 * folder picker (read access to that folder alone, asked once per folder),
 * when the copies run (Wi-Fi and charging by default, or every few hours),
 * which computer, whether deletions are mirrored (off), and Run now. A
 * folder's own line says how its last run went.
 */
@Composable
fun KeptFoldersSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val folders = (version + resumes).let { KeptFolders.folders(context) }
    val storageName = stringResource(R.string.phone_folders_storage)
    val notKept = stringResource(R.string.kept_not_kept)
    val noComputer = stringResource(R.string.kept_no_computer)
    val started = stringResource(R.string.kept_run_started)

    val pick =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree: Uri? ->
            if (tree != null) {
                val id = runCatching { DocumentsContract.getTreeDocumentId(tree) }.getOrNull()
                if (!KeptFolders.add(context, tree, KeptRule.label(id, storageName))) {
                    Toast.makeText(context, notKept, Toast.LENGTH_LONG).show()
                }
                version++
            }
        }

    val title = stringResource(R.string.kept_title)
    val summary = stringResource(if (folders.isEmpty()) R.string.kept_empty else R.string.kept_summary)
    SettingRow(title, summary)
    val add = stringResource(R.string.kept_add)
    SettingsBlock(title, summary, add, *folders.map { it.label }.toTypedArray()) {
        folders.forEach { folder ->
            val status = KeptFolders.status(context, folder.id)
            val lost = !KeptFolders.readable(context, folder)
            val line =
                when {
                    lost -> stringResource(R.string.kept_access_lost, folder.label)
                    status == null -> stringResource(R.string.kept_status_never)
                    else ->
                        stringResource(
                            R.string.kept_status_run,
                            DateUtils.getRelativeTimeSpanString(status.atMs, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS).toString(),
                            status.sent,
                            status.failed,
                        )
                }
            val note = if (lost) null else status?.note
            ListItem(
                headlineContent = { Text(folder.label) },
                supportingContent = { Text(if (note != null) "$line\n$note" else line) },
                modifier = Modifier.padding(start = 24.dp),
                trailingContent = {
                    val removeLabel = stringResource(R.string.kept_remove_named, folder.label)
                    TextButton(
                        onClick = {
                            KeptFolders.remove(context, folder)
                            version++
                        },
                        modifier = Modifier.semantics { contentDescription = removeLabel },
                    ) { Text(stringResource(R.string.kept_remove)) }
                },
            )
        }
    }
    SettingLink(add, null) { runCatching { pick.launch(null) } }

    if (folders.isEmpty()) return

    // When: Wi-Fi and charging, or every few hours.
    val modes = KeptRule.Mode.entries
    val mode = (version + resumes).let { KeptFolders.mode(context) }
    val scheduleTitle = stringResource(R.string.kept_schedule)
    val scheduleSummary = stringResource(R.string.kept_schedule_summary)
    val everySummary = stringResource(R.string.kept_every_summary)
    if (rememberSettingShown(scheduleTitle, scheduleSummary, everySummary)) {
        SettingRow(scheduleTitle, scheduleSummary)
        ChoiceRow(
            null,
            listOf(stringResource(R.string.kept_mode_charging), stringResource(R.string.kept_mode_every)),
            modes.indexOf(mode),
            horizontal = 16.dp,
        ) {
            KeptFolders.setSchedule(context, modes[it], KeptFolders.hours(context))
            version++
        }
        if (mode == KeptRule.Mode.EVERY) {
            val hours = KeptRule.periodHours(mode, KeptFolders.hours(context))
            SettingRow(everySummary, null)
            ChoiceRow(
                null,
                KeptRule.HOURS.map { stringResource(R.string.kept_hours, it) },
                KeptRule.HOURS.indexOf(hours),
                horizontal = 16.dp,
            ) {
                KeptFolders.setSchedule(context, mode, KeptRule.HOURS[it])
                version++
            }
        }
    }

    // Which computer, when this phone has more than one saved.
    val hosts = HostStore.hosts(context)
    if (hosts.size > 1) {
        val chosen = KeptFolders.computer(context)
        val own = dev.pocketdesk.android.notify.PhoneLink.target(context)
        val current = hosts.firstOrNull { it.address == chosen } ?: own
        val currentName = current?.let { HostStore.shownName(context, it.address) }.orEmpty()
        SettingLink(
            stringResource(R.string.kept_computer),
            stringResource(R.string.kept_computer_own, currentName) + "\n" + stringResource(R.string.kept_computer_summary),
        ) {
            val index = hosts.indexOfFirst { it.address == current?.address }
            KeptFolders.setComputer(context, hosts[(index + 1) % hosts.size].address)
            version++
        }
    }

    SettingSwitch(
        stringResource(R.string.kept_mirror),
        stringResource(R.string.kept_mirror_summary),
        (version + resumes).let { KeptFolders.mirror(context) },
        {
            KeptFolders.setMirror(context, it)
            version++
        },
    )
    SettingLink(stringResource(R.string.kept_run_now), stringResource(R.string.kept_run_now_summary)) {
        val ok = KeptFolders.start(context)
        Toast.makeText(context, if (ok) started else noComputer, Toast.LENGTH_SHORT).show()
    }
}
