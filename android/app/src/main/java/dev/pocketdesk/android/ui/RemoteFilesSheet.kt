package dev.pocketdesk.android.ui

import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.service.TransferService
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** The computer's browse reply (F7b) as a [RemoteFolder]. */
fun parseRemoteFolder(json: String): RemoteFolder {
    val o = JSONObject(json)
    val files = o.optJSONArray("files") ?: JSONArray()
    val entries = (0 until files.length()).mapNotNull { i ->
        val f = files.optJSONObject(i) ?: return@mapNotNull null
        val path = f.optString("path")
        if (path.isEmpty()) return@mapNotNull null
        RemoteEntry(
            name = f.optString("name"),
            path = path,
            isDirectory = f.optBoolean("isDirectory"),
            size = f.optLong("size"),
            modified = f.optLong("modified"),
        )
    }
    return RemoteFolder(
        path = o.optString("path"),
        parent = if (o.isNull("parent")) null else o.optString("parent").ifEmpty { null },
        entries = RemoteFiles.sorted(entries),
        error = if (o.isNull("error")) null else o.optString("error").ifEmpty { null },
    )
}

/**
 * The computer's home folder, browsed from the session (F7b). Folders open
 * in place, Up goes back, and a file is downloaded to Downloads/Pocketdesk
 * after a confirm that shows its size.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RemoteFilesSheet(address: String, controlPort: Int, onDismiss: () -> Unit) {
    val context = LocalContext.current
    // "" is the home folder.
    var path by remember { mutableStateOf("") }
    var folder by remember { mutableStateOf<RemoteFolder?>(null) }
    // The last folder that loaded, so a failed load still has a way back.
    var lastGood by remember { mutableStateOf<RemoteFolder?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var confirm by remember { mutableStateOf<RemoteEntry?>(null) }

    LaunchedEffect(address, controlPort, path) {
        folder = null
        failure = null
        val result = withContext(Dispatchers.IO) {
            RustCore.listRemoteFiles(address, controlPort, path).mapCatching(::parseRemoteFolder)
        }
        result.onSuccess {
            folder = it
            if (it.error == null) lastGood = it
        }.onFailure { failure = it.message ?: it.toString() }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(modifier = Modifier.padding(horizontal = 8.dp)) {
            Text(
                stringResource(R.string.row_files).trimEnd('…'),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(start = 8.dp),
            )
            val here = folder?.path?.let(RemoteFiles::title).orEmpty()
            if (here.isNotEmpty()) {
                Text(
                    here,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(start = 8.dp, bottom = 4.dp),
                )
            }
            val parent = folder?.parent ?: if (failure != null) RemoteFiles.wayBack(path, lastGood) else null
            if (parent != null) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.files_up)) },
                    leadingContent = { LlIcon(LlIcons.ArrowBack, null) },
                    modifier = Modifier.clickable { path = parent },
                )
            }
            HorizontalDivider()
            val shown = folder
            when {
                failure != null -> Message(stringResource(R.string.files_failed, failure.orEmpty()), error = true)
                shown == null -> Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 24.dp),
                    horizontalArrangement = Arrangement.Center,
                ) { CircularProgressIndicator() }
                shown.error != null -> Message(stringResource(R.string.files_failed, shown.error), error = true)
                shown.entries.isEmpty() -> Message(stringResource(R.string.files_empty), error = false)
                else -> LazyColumn(modifier = Modifier.padding(bottom = 24.dp)) {
                    items(shown.entries, key = { it.path }) { entry ->
                        ListItem(
                            headlineContent = {
                                Text(entry.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            },
                            supportingContent = if (entry.isDirectory) {
                                null
                            } else {
                                { Text(RemoteFiles.humanSize(entry.size)) }
                            },
                            leadingContent = {
                                LlIcon(if (entry.isDirectory) LlIcons.Folder else LlIcons.File, null)
                            },
                            modifier = Modifier.clickable {
                                if (entry.isDirectory) path = entry.path else confirm = entry
                            },
                        )
                    }
                }
            }
        }
    }

    confirm?.let { entry ->
        AlertDialog(
            onDismissRequest = { confirm = null },
            title = { Text(stringResource(R.string.files_download_title, entry.name)) },
            text = { Text(stringResource(R.string.files_download_text, RemoteFiles.humanSize(entry.size))) },
            confirmButton = {
                TextButton(onClick = {
                    confirm = null
                    download(context, address, controlPort, entry)
                }) { Text(stringResource(R.string.files_download)) }
            },
            dismissButton = {
                TextButton(onClick = { confirm = null }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

@Composable
private fun Message(text: String, error: Boolean) {
    Text(
        text,
        color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodyMedium,
        modifier = Modifier.padding(horizontal = 8.dp, vertical = 16.dp),
    )
}

private fun download(context: Context, address: String, controlPort: Int, entry: RemoteEntry) {
    TransferService.receive(context, address, controlPort, false, entry.path, entry.name, entry.size)
    Toast.makeText(context, context.getString(R.string.files_download_started, entry.name), Toast.LENGTH_SHORT).show()
}

/**
 * Files the computer's owner sent this phone (F7b): while the session is
 * up, each offer the bridge latched is downloaded to Downloads, with the
 * transfer notification as its progress and record.
 */
@Composable
fun FileOffersEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    LaunchedEffect(address, controlPort) {
        while (isActive) {
            val offers = withContext(Dispatchers.IO) { RustCore.takeFileOffers() }
            runCatching { JSONArray(offers) }.getOrNull()?.let { list ->
                for (i in 0 until list.length()) {
                    val offer = list.optJSONObject(i) ?: continue
                    val id = offer.optString("id").ifEmpty { continue }
                    TransferService.receive(
                        context,
                        address,
                        controlPort,
                        true,
                        id,
                        offer.optString("name").ifEmpty { "download" },
                        offer.optLong("size"),
                    )
                }
            }
            delay(1_000)
        }
    }
}
