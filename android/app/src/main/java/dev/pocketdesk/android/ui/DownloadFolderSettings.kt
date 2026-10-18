package dev.pocketdesk.android.ui

import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.service.DownloadTarget

/**
 * C2: where files from computers go, Downloads/Pocketdesk or a folder the
 * owner picks with Android's folder picker (a lasting write grant).
 */
@Composable
fun DownloadFolderSetting() {
    val context = LocalContext.current
    val resumes = rememberResumes()
    var version by remember { mutableIntStateOf(0) }
    val picked = Prefs.downloadTree(context)
    val tree = remember(resumes, version) { DownloadTarget.tree(context) }
    val lost = remember(resumes, version) { DownloadTarget.treeLost(context) }
    val pick =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { chosen: Uri? ->
            if (chosen != null) {
                val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                val kept = runCatching { context.contentResolver.takePersistableUriPermission(chosen, flags) }.isSuccess
                if (kept) {
                    // The folder before it is let go, so grants do not pile
                    // up; the same one picked again keeps its grant.
                    if (picked != chosen.toString()) release(context, picked)
                    Prefs.setDownloadTree(context, chosen.toString())
                }
                version++
            }
        }
    val where = when {
        tree != null -> DownloadTarget.treeName(context, tree) ?: stringResource(R.string.settings_download_folder_picked)
        else -> stringResource(R.string.settings_download_folder_default)
    }
    SettingLink(
        stringResource(R.string.settings_download_folder),
        if (lost) stringResource(R.string.settings_download_folder_lost) else where,
    ) { pick.launch(null) }
    if (picked != null) {
        SettingLink(stringResource(R.string.settings_download_folder_reset), null) {
            release(context, picked)
            Prefs.setDownloadTree(context, null)
            version++
        }
    }
}

/**
 * Let go of this setting's grant on [tree]: the write, and the read too
 * unless "Folders the computer may open" shares the folder (grants are per
 * folder, not per feature).
 */
private fun release(context: android.content.Context, tree: String?) {
    tree ?: return
    val shared = dev.pocketdesk.android.files.PhoneFolders.all(context).any { it.uri == tree }
    val flags = if (shared) {
        Intent.FLAG_GRANT_WRITE_URI_PERMISSION
    } else {
        Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
    }
    runCatching { context.contentResolver.releasePersistableUriPermission(tree.toUri(), flags) }
}
