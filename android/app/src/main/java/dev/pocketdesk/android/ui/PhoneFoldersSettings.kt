package dev.pocketdesk.android.ui

import android.net.Uri
import android.provider.DocumentsContract
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.core.net.toUri
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.files.PhoneFolders

/**
 * Settings "Folders the computer may open" (T10g): each folder picked with
 * Android's folder picker, with Remove; nothing else on the phone is
 * reachable from the computer. Under each, "The computer may add files"
 * (N6), which asks the picker again for a write grant to that folder.
 */
@Composable
fun PhoneFoldersSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val folders = version.let { PhoneFolders.all(context) }
    val storageName = stringResource(R.string.phone_folders_storage)
    val notKept = stringResource(R.string.phone_folders_not_kept)
    val addSame = stringResource(R.string.phone_folders_add_same)
    // N6: the folder a write grant is being asked for.
    var marking by remember { mutableStateOf<PhoneFolders.Folder?>(null) }
    val markPick =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree: Uri? ->
            val folder = marking
            marking = null
            if (tree != null && folder != null && !PhoneFolders.markAdd(context, folder, tree)) {
                Toast.makeText(context, addSame, Toast.LENGTH_LONG).show()
            }
            version++
        }
    val pick =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree: Uri? ->
            if (tree != null) {
                val name =
                    runCatching { DocumentsContract.getTreeDocumentId(tree).substringAfterLast(':').substringAfterLast('/') }
                        .getOrNull()
                        ?.ifEmpty { null }
                        ?: storageName
                if (!PhoneFolders.add(context, tree, name)) {
                    Toast.makeText(context, notKept, Toast.LENGTH_LONG).show()
                }
                version++
            }
        }

    SettingRow(
        stringResource(R.string.settings_phone_folders),
        stringResource(if (folders.isEmpty()) R.string.phone_folders_empty else R.string.settings_phone_folders_summary),
    )
    folders.forEach { folder ->
        ListItem(
            headlineContent = { Text(folder.name) },
            modifier = Modifier.padding(start = 24.dp),
            trailingContent = {
                // Named for TalkBack: which folder it removes (E6).
                val removeLabel = stringResource(R.string.phone_folders_remove_named, folder.name)
                TextButton(
                    onClick = {
                        PhoneFolders.remove(context, folder)
                        version++
                    },
                    modifier = Modifier.semantics { contentDescription = removeLabel },
                ) { Text(stringResource(R.string.phone_folders_remove)) }
            },
        )
        // N6: one row, one switch, one TalkBack stop.
        ListItem(
            headlineContent = { Text(stringResource(R.string.phone_folders_add_files)) },
            supportingContent = { Text(stringResource(R.string.phone_folders_add_files_summary)) },
            modifier = Modifier.padding(start = 40.dp),
            trailingContent = {
                val label = stringResource(R.string.phone_folders_add_files_named, folder.name)
                Switch(
                    checked = folder.add,
                    onCheckedChange = { on ->
                        if (on) {
                            marking = folder
                            // At the folder itself: the write grant is for it alone.
                            // The picker opens at a folder's document, not its tree
                            // (N6: on the OPPO it opened at the last folder instead).
                            val tree = folder.uri.toUri()
                            val at = runCatching {
                                DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
                            }.getOrDefault(tree)
                            runCatching { markPick.launch(at) }
                        } else {
                            PhoneFolders.unmarkAdd(context, folder)
                            version++
                        }
                    },
                    modifier = Modifier.semantics { contentDescription = label },
                )
            },
        )
    }
    // A row like its neighbours, as it opens the picker (E6).
    SettingLink(stringResource(R.string.phone_folders_add), null) { runCatching { pick.launch(null) } }
}
