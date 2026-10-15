package dev.pocketdesk.android.ui

import android.net.Uri
import android.provider.DocumentsContract
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.files.PhoneFolders

/**
 * Settings "Folders the computer may open" (T10g): each folder picked with
 * Android's folder picker, with Remove; nothing else on the phone is
 * reachable from the computer.
 */
@Composable
fun PhoneFoldersSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val folders = version.let { PhoneFolders.all(context) }
    val storageName = stringResource(R.string.phone_folders_storage)
    val pick =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree: Uri? ->
            if (tree != null) {
                val name =
                    runCatching { DocumentsContract.getTreeDocumentId(tree).substringAfterLast(':').substringAfterLast('/') }
                        .getOrNull()
                        ?.ifEmpty { null }
                        ?: storageName
                PhoneFolders.add(context, tree, name)
                version++
            }
        }

    Text(
        stringResource(R.string.settings_phone_folders),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 2.dp),
    )
    Text(
        stringResource(if (folders.isEmpty()) R.string.phone_folders_empty else R.string.settings_phone_folders_summary),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    folders.forEach { folder ->
        ListItem(
            headlineContent = { Text(folder.name) },
            trailingContent = {
                TextButton(onClick = {
                    PhoneFolders.remove(context, folder)
                    version++
                }) { Text(stringResource(R.string.phone_folders_remove)) }
            },
        )
    }
    TextButton(onClick = { runCatching { pick.launch(null) } }, modifier = Modifier.padding(horizontal = 8.dp)) {
        Text(stringResource(R.string.phone_folders_add))
    }
}
